use super::{
    Filter, Store,
    filter::{FAILED, GROUP_SELECTION, GROUP_TIMELINE_SELECTION},
    metrics::{Mean, TokenSum},
    value::{parse, text},
};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde_json::{Value, json};
use std::collections::HashMap;

// Counts describe retained definitions, not the filtered answers. Keep this map
// inside one read snapshot; family IDs and strict keys are separate namespaces.
pub(super) type VersionCounts = HashMap<(bool, String, String), i64>;

pub(super) fn distribution_label(group: &Value, raw: &str) -> String {
    match group["kind"].as_str() {
        Some("noul") => {
            let low = raw.parse::<f64>().unwrap_or(0.0) / 10.0;
            if raw == "9" {
                "0.9–1.0".into()
            } else {
                format!("{low:.1}–<{:.1}", low + 0.1)
            }
        }
        Some("score") => {
            let low = raw.parse::<usize>().unwrap_or(0);
            let maximum = group["definition"]["criteria"]
                .as_array()
                .or_else(|| group["definition"]["levels"].as_array())
                .map(|a| a.len().saturating_sub(1));
            if maximum == Some(low) {
                raw.into()
            } else {
                format!("{low}–<{}", low + 1)
            }
        }
        _ => raw.into(),
    }
}

pub(super) fn distribution_bin(group: &Value, raw: &str, count: Value) -> Value {
    if group["kind"] == "choice"
        && group["adapter"] == "openai-decisions-v1"
        && let Some(bin) =
            crate::model::decisions::typed_bin(&group["definition"], raw, count.clone())
    {
        return bin;
    }
    json!({"label":distribution_label(group,raw),"count":count})
}

impl Store {
    pub(super) fn version_count_query(family: bool) -> &'static str {
        // Separate predicates let SQLite use the complete source/key index or
        // the family index instead of scanning every group from the source.
        if family {
            "SELECT count(*) FROM groups WHERE source=? AND family_id=?"
        } else {
            "SELECT count(*) FROM groups WHERE source=? AND key=?"
        }
    }

    pub(super) fn group_metadata(
        conn: &Connection,
        id: &str,
        version_counts: &mut VersionCounts,
    ) -> Result<Option<Value>> {
        let meta = conn.query_row("SELECT id,key,kind,source,definition_id,presentation_id,definition,task_version,family_id,family_name,adapter FROM groups WHERE id=? OR family_id=? ORDER BY id LIMIT 1",params![id,id], |r| Ok(json!({"id":id,"key":r.get::<_,String>(1)?,"name":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"source":r.get::<_,String>(3)?,"definition_id":r.get::<_,String>(4)?,"presentation_id":r.get::<_,String>(5)?,"definition":parse(r.get(6)?)?,"task_version":r.get::<_,Option<String>>(7)?,"family_id":r.get::<_,Option<String>>(8)?,"family_name":r.get::<_,Option<String>>(9)?,"adapter":r.get::<_,Option<String>>(10)?}))).optional()?;
        let Some(mut result) = meta else {
            return Ok(None);
        };
        let family = result["family_id"].as_str() == Some(id);
        if family {
            result["name"] = result["family_name"].clone();
        }
        result["is_family"] = json!(family);
        let scope = (
            family,
            text(&result, "source"),
            if family {
                id.to_owned()
            } else {
                text(&result, "key")
            },
        );
        let version_count = match version_counts.entry(scope) {
            std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let (_, source, selector) = entry.key();
                let count = conn.query_row(
                    Self::version_count_query(family),
                    params![source, selector],
                    |r| r.get::<_, i64>(0),
                )?;
                *entry.insert(count)
            }
        };
        result["version_count"] = json!(version_count);
        Ok(Some(result))
    }

    // These conservative cutoffs are planning heuristics, not result limits.
    // Inspect at most 16,384 covering-index entries per source/model scope,
    // independently, without parsing saved bodies. Small date/filter scopes
    // keep the original plan; otherwise consider at most a quarter of their
    // candidates, capped at 4,096 group answers. Residual search/error/group
    // predicates do not provide a narrower indexed parent entry point.
    pub(super) fn group_lookup_limit(conn: &Connection, filter: &Filter) -> Result<usize> {
        let mut scopes = Vec::new();
        for (field, value, index) in [
            ("source", &filter.source, "request_source_time"),
            ("model", &filter.model, "request_model_time"),
        ] {
            if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                scopes.push((Some((field, value)), index));
            }
        }
        if scopes.is_empty() {
            scopes.push((None, "request_time"));
        }
        let mut candidates = 16_384_i64;
        for (selection, index) in scopes {
            let mut scope = filter.clone();
            scope.source = None;
            scope.model = None;
            scope.search = None;
            scope.status = None;
            scope.group = None;
            if let Some((field, value)) = selection {
                if field == "source" {
                    scope.source = Some(value.clone());
                } else {
                    scope.model = Some(value.clone());
                }
            }
            let (predicate, args) = Self::predicate(&scope)?;
            let count: i64 = conn.query_row(
                &format!("SELECT count(*) FROM (SELECT 1 FROM requests r INDEXED BY {index} WHERE {predicate} LIMIT 16384)"),
                params_from_iter(args), |row| row.get(0),
            )?;
            candidates = candidates.min(count);
            if candidates < 512 {
                return Ok(0);
            }
        }
        Ok((candidates / 4).min(4096) as usize)
    }

    pub(super) fn group_has_small_membership(
        conn: &Connection,
        id: &str,
        limit: usize,
    ) -> Result<bool> {
        if limit == 0 {
            return Ok(false);
        }
        // An OFFSET existence probe needs at most limit+1 answer entries; an
        // exact count would do more work. Answers conservatively bound distinct
        // parents, including families with several answers from one request.
        let oversized: bool = conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM groups g CROSS JOIN answers a ON a.group_id=g.id WHERE g.id=? OR g.family_id=? LIMIT 1 OFFSET {limit})"),
            params![id, id], |row| row.get(0),
        )?;
        Ok(!oversized)
    }

    fn group_answer_join(group_first: bool) -> &'static str {
        if group_first {
            "FROM groups g CROSS JOIN answers a ON a.group_id=g.id CROSS JOIN requests r ON r.id=a.request_id"
        } else {
            "FROM answers a JOIN requests r ON r.id=a.request_id JOIN groups g ON g.id=a.group_id"
        }
    }

    pub(super) fn group_summary(
        conn: &Connection,
        id: &str,
        filter: &Filter,
        lookup_limit: usize,
        mut result: Value,
    ) -> Result<(Value, bool)> {
        let family = result["is_family"].as_bool().unwrap_or(false);
        let (predicate, mut args) = Self::predicate(filter)?;
        let selector = if family { "g.family_id=?" } else { "g.id=?" };
        args.push(id.to_owned().into());
        let group_first = Self::group_has_small_membership(conn, id, lookup_limit)?;
        let answer_join = Self::group_answer_join(group_first);
        let from = format!("{answer_join} WHERE {predicate} AND {selector}");
        let (count,valid,requests,last,mean,confidence): (i64,i64,i64,Option<i64>,Option<f64>,Option<f64>) = conn.query_row(&format!("SELECT count(*),coalesce(sum(a.valid),0),count(DISTINCT r.id),max(r.timestamp),avg(CASE WHEN a.valid THEN a.value_num END),avg(CASE WHEN a.valid THEN a.confidence END) {from}"), params_from_iter(&args), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
        result["answer_count"] = json!(count);
        result["valid_count"] = json!(valid);
        result["request_count"] = json!(requests);
        result["last_seen"] = json!(last);
        result["mean_value"] = json!(mean);
        result["mean_confidence"] = json!(confidence);
        result.as_object_mut().unwrap().extend(Self::group_metrics(
            conn,
            id,
            family,
            filter,
            group_first,
        )?);
        let kind = text(&result, "kind");
        let expression = match kind.as_str() {
            "choice" => "a.value_text",
            "noul" => "min(9,CAST(a.value_num*10 AS INTEGER))",
            _ => "CAST(a.value_num AS INTEGER)",
        };
        let mut stmt = conn.prepare(&format!(
            "SELECT CAST({expression} AS TEXT),count(*) {from} AND a.valid=1 AND a.kind!='box' GROUP BY 1 ORDER BY {expression}"
        ))?;
        let distribution = stmt
            .query_map(params_from_iter(&args), |r| {
                let raw: String = r.get(0)?;
                Ok(distribution_bin(&result, &raw, json!(r.get::<_, i64>(1)?)))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        result["distribution"] = json!(distribution);
        Ok((result, group_first))
    }

    pub(super) fn group_metrics(
        conn: &Connection,
        id: &str,
        family: bool,
        filter: &Filter,
        group_first: bool,
    ) -> Result<serde_json::Map<String, Value>> {
        let (predicate, mut args) = Self::predicate(filter)?;
        args.push(id.to_owned().into());
        let selector = if family { "g.family_id=?" } else { "g.id=?" };
        let (parent_predicate, mut parent_args) = Self::predicate(filter)?;
        parent_args.insert(0, id.to_owned().into());
        let parent_selector = if family { "gg.family_id=?" } else { "gg.id=?" };
        // Start with distinct parents from this group. With an IN predicate,
        // a source/model filter can make SQLite scan that entire population
        // once per version instead. CROSS JOIN keeps these indexed parent
        // lookups first while preserving one contribution per request.
        let parent_scope = format!(
            "FROM (SELECT DISTINCT aa.request_id FROM answers aa JOIN groups gg ON gg.id=aa.group_id WHERE {parent_selector}) selected CROSS JOIN requests r ON r.id=selected.request_id WHERE {parent_predicate}"
        );
        let metrics = conn.query_row(
            &format!("SELECT avg(duration_ms),sum(cost_usd),count(cost_usd),sum(input_tokens),sum(output_tokens),coalesce(sum({FAILED}),0),count(input_tokens),count(output_tokens) {parent_scope}"),
            params_from_iter(&parent_args), |row| Ok(json!({
                "mean_latency_ms":row.get::<_,Option<f64>>(0)?, "cost_usd":row.get::<_,Option<f64>>(1)?,
                "cost_known_requests":row.get::<_,i64>(2)?, "input_tokens":row.get::<_,Option<i64>>(3)?,
                "output_tokens":row.get::<_,Option<i64>>(4)?, "error_count":row.get::<_,i64>(5)?,
                "input_tokens_known_requests":row.get::<_,i64>(6)?,
                "output_tokens_known_requests":row.get::<_,i64>(7)?,
                "input_tokens_overflow":false, "output_tokens_overflow":false
            })),
        );
        let metrics = match metrics {
            Ok(metrics) => metrics,
            // Bundled SQLite reports SUM integer overflow with this exact code
            // and message. Keep its faster aggregate for all ordinary queries;
            // other database errors must still propagate to the caller.
            Err(rusqlite::Error::SqliteFailure(error, Some(message)))
                if error.extended_code == rusqlite::ffi::SQLITE_ERROR
                    && message == "integer overflow" =>
            {
                let mut parent_stmt = conn.prepare(&format!(
                    "SELECT duration_ms,cost_usd,input_tokens,output_tokens,{FAILED} {parent_scope}"
                ))?;
                let mut parents = parent_stmt.query(params_from_iter(&parent_args))?;
                let (mut latency, mut cost) = (Mean::default(), Mean::default());
                let (mut input, mut output) = (TokenSum::default(), TokenSum::default());
                let mut errors = 0_i64;
                // The parent subquery deduplicates requests shared by multiple answers.
                // Keep coverage and unrelated metrics even when a token sum overflows.
                while let Some(row) = parents.next()? {
                    if let Some(value) = row.get::<_, Option<f64>>(0)? {
                        latency.add(value);
                    }
                    if let Some(value) = row.get::<_, Option<f64>>(1)? {
                        cost.add(value);
                    }
                    input.add(row.get(2)?);
                    output.add(row.get(3)?);
                    errors += i64::from(row.get::<_, Option<bool>>(4)?.unwrap_or(false));
                }
                json!({
                    "mean_latency_ms":latency.value(), "cost_usd":cost.total(),
                    "cost_known_requests":cost.count, "input_tokens":input.total,
                    "output_tokens":output.total, "error_count":errors,
                    "input_tokens_known_requests":input.known,
                    "output_tokens_known_requests":output.known,
                    "input_tokens_overflow":input.overflow,
                    "output_tokens_overflow":output.overflow
                })
            }
            Err(error) => return Err(error.into()),
        };
        // Labels are unique by (request_id,key), so this join preserves answer
        // multiplicity while computing reviews and warnings in the same pass.
        let answer_join = Self::group_answer_join(group_first);
        let answer_metrics: Value = conn.query_row(
            &format!("SELECT coalesce(sum(coalesce(json_array_length(json_extract(a.data,'$.warnings')),0)>0),0),coalesce(sum(l.label='correct'),0),coalesce(sum(l.label='incorrect'),0),coalesce(sum(l.label='unknown'),0),coalesce(sum(l.label IS NULL),0) {answer_join} LEFT JOIN labels l ON l.request_id=a.request_id AND l.key=a.key WHERE {predicate} AND {selector}"),
            params_from_iter(&args), |row| Ok(json!({
                "warning_count":row.get::<_,i64>(0)?,
                "review_counts":{
                    "correct":row.get::<_,i64>(1)?,"incorrect":row.get::<_,i64>(2)?,
                    "unknown":row.get::<_,i64>(3)?,"unlabeled":row.get::<_,i64>(4)?
                }
            })),
        )?;
        let mut result = metrics.as_object().unwrap().clone();
        result.extend(answer_metrics.as_object().unwrap().clone());
        Ok(result)
    }

    pub fn group(&self, id: &str, filter: &Filter) -> Result<Option<Value>> {
        let mut anchored = filter.clone();
        anchored
            .as_of
            .get_or_insert_with(|| Utc::now().timestamp_millis());
        let filter = &anchored;
        let mut connection = self.reader()?;
        let conn = connection.transaction()?;
        Self::group_from(&conn, id, filter)
    }

    pub(super) fn group_from(
        conn: &rusqlite::Transaction<'_>,
        id: &str,
        filter: &Filter,
    ) -> Result<Option<Value>> {
        let mut version_counts = VersionCounts::new();
        let Some(metadata) = Self::group_metadata(conn, id, &mut version_counts)? else {
            return Ok(None);
        };
        let lookup_limit = Self::group_lookup_limit(conn, filter)?;
        let (group, group_first) = Self::group_summary(conn, id, filter, lookup_limit, metadata)?;
        // The requested group narrows the caller's parent scope. Replacing an
        // existing group filter would make feeds and observations disagree
        // with the aggregate above when parents contain multiple groups.
        let (mut predicate, mut args) = Self::predicate(filter)?;
        if group_first {
            // Membership was bounded by at most 4096 answer rows. Keep only
            // matching integer parent IDs/timestamps for repeated detail reads.
            conn.execute_batch("CREATE TEMP TABLE group_request_ids(seq INTEGER PRIMARY KEY,timestamp INTEGER NOT NULL)")?;
            args.push(id.to_owned().into());
            args.push(id.to_owned().into());
            conn.execute(
                &format!("INSERT OR IGNORE INTO temp.group_request_ids SELECT r.seq,r.timestamp FROM groups g CROSS JOIN answers a ON a.group_id=g.id CROSS JOIN requests r ON r.id=a.request_id WHERE {predicate} AND (g.id=? OR g.family_id=?)"),
                params_from_iter(&args),
            )?;
            predicate = GROUP_SELECTION.into();
            args.clear();
        } else if filter.group.as_deref() != Some(id) {
            predicate.push_str(" AND EXISTS(SELECT 1 FROM answers ga JOIN groups gg ON gg.id=ga.group_id WHERE ga.request_id=r.id AND (gg.id=? OR gg.family_id=?))");
            args.push(id.to_owned().into());
            args.push(id.to_owned().into());
        }
        let requests = Self::request_summaries(conn, &predicate, &args, 100)?;
        let timeline_predicate = if group_first {
            GROUP_TIMELINE_SELECTION
        } else {
            &predicate
        };
        let (timeline, timeline_meta) = Self::timeline(conn, timeline_predicate, &args, filter)?;
        let family = group["is_family"].as_bool().unwrap_or(false);
        let (version_scope, mut version_args) = Self::predicate(filter)?;
        version_args.extend([
            text(&group, "source").into(),
            (family as i64).into(),
            id.to_owned().into(),
            (family as i64).into(),
            text(&group, "key").into(),
            id.to_owned().into(),
        ]);
        // Keep the viewed version in the bounded comparison list, then prefer
        // versions with the most recent activity in the caller's parent scope.
        let mut stmt = conn.prepare(&format!("SELECT g.id FROM groups g LEFT JOIN answers a ON a.group_id=g.id LEFT JOIN requests r ON r.id=a.request_id AND {version_scope} WHERE g.source=? AND ((?=1 AND g.family_id=?) OR (?=0 AND g.key=?)) GROUP BY g.id ORDER BY (g.id=?) DESC,max(r.timestamp) DESC,g.id LIMIT 100"))?;
        let ids = stmt
            .query_map(params_from_iter(&version_args), |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut versions = Vec::new();
        for id in ids {
            if group["id"] == id {
                versions.push(group.clone());
                continue;
            }
            if let Some(metadata) = Self::group_metadata(conn, &id, &mut version_counts)? {
                let (version, _) = Self::group_summary(conn, &id, filter, lookup_limit, metadata)?;
                versions.push(version);
            }
        }
        let mut answer_args = args.clone();
        answer_args.push(id.to_owned().into());
        answer_args.push(id.to_owned().into());
        let answer_join = Self::group_answer_join(group_first);
        // IN can combine the request-id and rowid constraints into a repeated
        // scan of the selected set. Check its integer primary key directly.
        let observation_predicate = if group_first {
            "EXISTS(SELECT 1 FROM temp.group_request_ids selected WHERE selected.seq=r.seq)"
        } else {
            &predicate
        };
        let mut stmt=conn.prepare(&format!("SELECT a.request_id,json_extract(r.data,'$.timestamp'),r.source,a.key,a.data,l.label,json_extract(r.data,'$.imported_at') {answer_join} LEFT JOIN labels l ON l.request_id=a.request_id AND l.key=a.key WHERE {observation_predicate} AND (g.id=? OR g.family_id=?) ORDER BY r.timestamp DESC,r.seq DESC,a.rowid LIMIT 100"))?;
        let answers = stmt
            .query_map(params_from_iter(&answer_args), |r| {
                let mut a = parse(r.get(4)?)?;
                a["request_id"] = json!(r.get::<_, String>(0)?);
                a["timestamp"] = json!(r.get::<_, Option<i64>>(1)?);
                a["source"] = json!(r.get::<_, String>(2)?);
                a["label"] = json!(r.get::<_, Option<String>>(5)?);
                a["imported_at"] = json!(r.get::<_, Option<i64>>(6)?);
                Ok(a)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(
            json!({"total_answers":group["answer_count"],"group":group,"versions":versions,"timeline":timeline,"timeline_meta":timeline_meta,"requests":requests,"answers":answers,"detail_limit":100}),
        ))
    }
}
