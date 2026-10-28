use super::{
    Filter, Store,
    groups::{VersionCounts, distribution_bin},
    metrics::{Mean, TimelineBucket, TokenSum},
    value::text,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{Connection, params, params_from_iter, types::Value as SqlValue};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub(super) struct DashboardRequests {
    pub(super) sample: bool,
    pub(super) summary: Value,
    pub(super) timeline: Vec<Value>,
    pub(super) parents: HashMap<String, i64>,
    pub(super) timeline_meta: Value,
}

#[derive(Default)]
struct GroupAnswers<'a> {
    count: i64,
    valid: i64,
    requests: i64,
    last: Option<i64>,
    previous_request: Option<&'a str>,
    values: Mean,
    confidence: Mean,
    numeric_bins: BTreeMap<i64, i64>,
    choice_bins: BTreeMap<String, i64>,
}

struct AnswerValues<'a> {
    valid: bool,
    number: Option<f64>,
    confidence: Option<f64>,
    choice: Option<&'a str>,
}

impl<'a> GroupAnswers<'a> {
    fn add(
        &mut self,
        kind: &str,
        request: &'a str,
        timestamp: i64,
        answer: AnswerValues<'_>,
    ) -> Result<()> {
        self.count += 1;
        if self.previous_request != Some(request) {
            self.requests += 1;
            self.previous_request = Some(request);
        }
        self.last = Some(self.last.map_or(timestamp, |last| last.max(timestamp)));
        if !answer.valid {
            return Ok(());
        }
        self.valid += 1;
        if let Some(number) = answer.number {
            self.values.add(number);
        }
        if let Some(confidence) = answer.confidence {
            self.confidence.add(confidence);
        }
        if kind == "choice" {
            let choice = answer.choice.context("Valid choice value is missing")?;
            if let Some(count) = self.choice_bins.get_mut(choice) {
                *count += 1;
            } else {
                self.choice_bins.insert(choice.to_owned(), 1);
            }
        } else if matches!(kind, "noul" | "score") {
            let number = answer.number.context("Valid numeric value is missing")?;
            let bin = if kind == "noul" {
                ((number * 10.0) as i64).min(9)
            } else {
                number as i64
            };
            *self.numeric_bins.entry(bin).or_default() += 1;
        }
        Ok(())
    }

    fn finish(self, kind: &str) -> Value {
        let bins: Vec<_> = if kind == "choice" {
            self.choice_bins.into_iter().collect()
        } else {
            self.numeric_bins
                .into_iter()
                .map(|(bin, count)| (bin.to_string(), count))
                .collect()
        };
        json!({
            "answer_count":self.count,"valid_count":self.valid,"request_count":self.requests,
            "last_seen":self.last,"mean_value":self.values.value(),
            "mean_confidence":self.confidence.value(),
            "distribution":bins.into_iter().map(|(bin,count)| json!({"label":bin,"count":count})).collect::<Vec<_>>()
        })
    }
}

struct SparseAnswer<'a> {
    group: String,
    group_kind: String,
    request: &'a str,
    timestamp: i64,
    kind: String,
    valid: i64,
    number: Option<f64>,
    choice: Option<String>,
    confidence: Option<f64>,
}

fn sqlite_number_order(left: Option<f64>, right: Option<f64>) -> std::cmp::Ordering {
    match (left, right) {
        // SQLite treats signed zeros as equal and stores NaN as NULL.
        (Some(left), Some(right)) => left
            .partial_cmp(&right)
            .unwrap_or(std::cmp::Ordering::Equal),
        _ => left.is_some().cmp(&right.is_some()),
    }
}

fn retain_dashboard_group(
    selected: &mut Vec<(String, Value)>,
    id: String,
    stats: GroupAnswers<'_>,
    kind: &str,
    after: &Option<(i64, String)>,
) {
    let Some(last) = stats.last else {
        return;
    };
    if after
        .as_ref()
        .is_some_and(|(time, cursor_id)| last > *time || (last == *time && id <= *cursor_id))
    {
        return;
    }
    let position = selected.partition_point(|(other_id, other)| {
        other["last_seen"].as_i64().unwrap() > last
            || (other["last_seen"] == last && other_id < &id)
    });
    if position < 101 {
        selected.insert(position, (id, stats.finish(kind)));
        selected.truncate(101);
    }
}

// A canonical group digest occupies 32 index bytes instead of 73 text bytes.
// Noncanonical IDs retain TEXT identity, including embedded NULs. SQLite BLOB
// and TEXT compare differently, so the transformation is injective. Preserve
// every remaining index column/order for deterministic floating-point sums.
// Built-in SQL functions keep the additive index writable by v0.3.0 too.
pub(super) const STRICT_ANSWER_QUERY: &str = "SELECT request_id,valid,value_num,confidence,value_text FROM answers INDEXED BY answer_overview WHERE (CASE WHEN length(group_id)=73 AND length(CAST(group_id AS BLOB))=73 AND substr(group_id,1,9)='group_v1_' AND substr(group_id,10) NOT GLOB '*[^0-9a-f]*' THEN unhex(substr(group_id,10)) ELSE group_id END)=(CASE WHEN length(?1)=73 AND length(CAST(?1 AS BLOB))=73 AND substr(?1,1,9)='group_v1_' AND substr(?1,10) NOT GLOB '*[^0-9a-f]*' THEN unhex(substr(?1,10)) ELSE ?1 END) ORDER BY request_id";

pub(super) const SPARSE_ANSWER_QUERY: &str = "SELECT a.group_id,g.kind,a.kind,a.valid,a.value_num,a.value_text,a.confidence,g.id IS NOT NULL AND g.family_id IS NULL AND instr(lower(g.key || ' ' || g.source || ' ' || g.kind || ' ' || g.id || ' ' || coalesce(g.task_version,'')),lower(?1))>0 FROM answers a INDEXED BY sqlite_autoindex_answers_1 LEFT JOIN groups g ON g.id=a.group_id WHERE a.request_id=?2 LIMIT ?3";

impl Store {
    /// Scan compact request metrics once for every overview component. The
    /// covering index avoids reading/parsing saved JSON bodies. All values and
    /// parent membership come from this transaction, including imported events.
    pub(super) fn dashboard_requests(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        filter: &Filter,
    ) -> Result<DashboardRequests> {
        let spec = Self::timeline_spec(conn, predicate, args, filter)?;
        let mut stmt = conn.prepare(&format!(
            "SELECT id,timestamp,event_kind,answer_count,status,input_tokens,output_tokens,cost_usd,sample,capture_complete,duration_ms,json_extract(data,'$.timestamp'),json_extract(data,'$.transport_error') FROM requests r INDEXED BY request_dashboard WHERE {predicate}"
        ))?;
        let mut rows = stmt.query(params_from_iter(args))?;
        let mut parents = HashMap::new();
        let mut sample = false;
        let mut buckets = BTreeMap::<i64, TimelineBucket>::new();
        let mut durations = Vec::new();
        let (mut count, mut answers, mut errors, mut actions, mut samples, mut incomplete) =
            (0_i64, 0_i64, 0_i64, 0_i64, 0_i64, 0_i64);
        let (mut input, mut output) = (TokenSum::default(), TokenSum::default());
        let mut cost = Mean::default();
        while let Some(row) = rows.next()? {
            let timestamp: i64 = row.get(1)?;
            parents.insert(row.get::<_, String>(0)?, timestamp);
            // Imported action receipts can also be synthetic. The overview
            // flag covers every selected event; summary counts stay scoped
            // to inference requests below.
            let is_sample = row.get::<_, bool>(8)?;
            sample |= is_sample;
            if row.get_ref(2)?.as_str()? != "request" {
                actions += 1;
                continue;
            }
            count += 1;
            answers = answers
                .checked_add(row.get(3)?)
                .context("integer overflow")?;
            let error = row
                .get::<_, Option<i64>>(4)?
                .is_some_and(|status| status >= 400)
                || row.get_ref(12)? != rusqlite::types::ValueRef::Null;
            errors += i64::from(error);
            let input_tokens: Option<i64> = row.get(5)?;
            let output_tokens: Option<i64> = row.get(6)?;
            input.add(input_tokens);
            output.add(output_tokens);
            let charge: Option<f64> = row.get(7)?;
            if let Some(charge) = charge {
                cost.add(charge);
            }
            samples += i64::from(is_sample);
            incomplete += i64::from(!row.get::<_, bool>(9)?);
            let duration: Option<f64> = row.get(10)?;
            if let Some(duration) = duration {
                durations.push(duration);
            }
            // Storage timestamp may be imported_at. It is not evidence of an
            // original event time, so unknown timestamps never enter timelines.
            if row.get_ref(11)? != rusqlite::types::ValueRef::Null {
                let bucket = buckets.entry(spec.bucket(timestamp)).or_default();
                bucket.requests += 1;
                bucket.errors += i64::from(error);
                if let Some(duration) = duration {
                    bucket.latency.add(duration);
                }
                if let Some(charge) = charge {
                    bucket.cost.add(charge);
                }
            }
        }
        let mut summary = json!({
            "request_count":count,"answer_count":answers,"error_count":errors,
            "input_tokens":input.total,"output_tokens":output.total,"cost_usd":cost.total(),
            "input_tokens_known_requests":input.known,"output_tokens_known_requests":output.known,
            "input_tokens_overflow":input.overflow,"output_tokens_overflow":output.overflow,
            "cost_known_requests":cost.count,"sample_count":samples,"incomplete_count":incomplete,
            "action_count":actions
        });
        // Exact nearest-rank percentiles need two selections, not a full sort.
        for (name, p) in [("p50_ms", 0.50), ("p95_ms", 0.95)] {
            summary[name] = if durations.is_empty() {
                Value::Null
            } else {
                let rank = ((durations.len() as f64 * p).ceil() as usize).saturating_sub(1);
                let (_, value, _) = durations.select_nth_unstable_by(rank, f64::total_cmp);
                json!(*value)
            };
        }
        let timeline = spec.finish(buckets);
        Ok(DashboardRequests {
            sample,
            summary,
            timeline,
            parents,
            timeline_meta: spec.metadata(),
        })
    }

    /// Use bounded parent lookups for sparse scopes and the covering group
    /// index for broad scopes. Both keep the same answer accumulation order.
    pub(super) fn strict_dashboard_groups(
        conn: &Connection,
        parents: &HashMap<String, i64>,
        filter: &Filter,
    ) -> Result<Vec<(String, Value)>> {
        if parents.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(groups) = Self::sparse_dashboard_groups(conn, parents, filter)? {
            return Ok(groups);
        }
        Self::broad_dashboard_groups(conn, parents, filter)
    }

    /// A sparse selection should not visit every retained group and answer.
    /// Bound the probe, copied text and answer count independently. The broad
    /// covering-index path remains preferable for large or dense selections.
    pub(super) fn sparse_dashboard_groups(
        conn: &Connection,
        parents: &HashMap<String, i64>,
        filter: &Filter,
    ) -> Result<Option<Vec<(String, Value)>>> {
        if parents.len() > 512 {
            return Ok(None);
        }
        let sparse: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM requests INDEXED BY request_time LIMIT 1 OFFSET ?)",
            [(parents.len() * 4) as i64],
            |row| row.get(0),
        )?;
        if !sparse {
            return Ok(None);
        }
        let mut statement = conn.prepare(SPARSE_ANSWER_QUERY)?;
        let mut answers = Vec::new();
        let mut visited = 0;
        let mut copied_bytes = 0;
        for (request, &timestamp) in parents {
            let mut rows = statement.query(params![
                filter.group_search.as_deref().unwrap_or(""),
                request,
                4097 - visited
            ])?;
            while let Some(row) = rows.next()? {
                visited += 1;
                if visited > 4096 {
                    return Ok(None);
                }
                // Count before applying family/search eligibility so even an
                // ineligible large parent cannot turn this into an open scan.
                if !row.get::<_, bool>(7)? {
                    continue;
                }
                let group = row.get_ref(0)?.as_str()?;
                let group_kind = row.get_ref(1)?.as_str()?;
                let Ok(kind) = row.get_ref(2)?.as_str() else {
                    // The original plan does not decode answer kinds. Keep
                    // unusual SQLite storage classes on that existing path.
                    return Ok(None);
                };
                let valid: i64 = row.get(3)?;
                let (number, choice, confidence) = if valid != 0 {
                    (
                        row.get::<_, Option<f64>>(4)?,
                        match row.get_ref(5)?.as_str_or_null() {
                            Ok(choice) => choice,
                            Err(_) => return Ok(None),
                        },
                        row.get::<_, Option<f64>>(6)?,
                    )
                } else {
                    (None, None, None)
                };
                // Choice labels and legacy IDs can be large. Do not copy an
                // unbounded record into the in-memory ordering buffer.
                for bytes in [
                    group.len(),
                    group_kind.len(),
                    kind.len(),
                    choice.map_or(0, str::len),
                ] {
                    if bytes > 1024 * 1024 - copied_bytes {
                        return Ok(None);
                    }
                    copied_bytes += bytes;
                }
                answers.push(SparseAnswer {
                    group: group.to_owned(),
                    group_kind: group_kind.to_owned(),
                    request,
                    timestamp,
                    kind: kind.to_owned(),
                    valid,
                    number,
                    choice: choice.map(str::to_owned),
                    confidence,
                });
            }
        }
        // Match answer_overview's complete tie order, including malformed
        // legacy groups with several keys/kinds in one parent. Sorting only
        // request IDs would change compensated sums and some exact means.
        answers.sort_by(|left, right| {
            left.group
                .cmp(&right.group)
                .then_with(|| left.request.cmp(right.request))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.valid.cmp(&right.valid))
                .then_with(|| sqlite_number_order(left.number, right.number))
                .then_with(|| left.choice.cmp(&right.choice))
                .then_with(|| sqlite_number_order(left.confidence, right.confidence))
        });
        let after = Self::group_cursor(filter)?;
        let mut selected = Vec::new();
        for group in answers.chunk_by(|left, right| left.group == right.group) {
            let mut stats = GroupAnswers::default();
            for answer in group {
                stats.add(
                    &answer.group_kind,
                    answer.request,
                    answer.timestamp,
                    AnswerValues {
                        valid: answer.valid != 0,
                        number: answer.number,
                        confidence: answer.confidence,
                        choice: answer.choice.as_deref(),
                    },
                )?;
            }
            retain_dashboard_group(
                &mut selected,
                group[0].group.clone(),
                stats,
                &group[0].group_kind,
                &after,
            );
        }
        Ok(Some(selected))
    }

    pub(super) fn broad_dashboard_groups(
        conn: &Connection,
        parents: &HashMap<String, i64>,
        filter: &Filter,
    ) -> Result<Vec<(String, Value)>> {
        let mut ids = conn.prepare("SELECT id,kind FROM groups WHERE family_id IS NULL AND instr(lower(key || ' ' || source || ' ' || kind || ' ' || id || ' ' || coalesce(task_version,'')),lower(?))>0 ORDER BY id")?;
        let after = Self::group_cursor(filter)?;
        let mut answers = conn.prepare(STRICT_ANSWER_QUERY)?;
        let mut selected: Vec<(String, Value)> = Vec::new();
        for row in ids.query_map([filter.group_search.as_deref().unwrap_or("")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (id, kind) = row?;
            let mut rows = answers.query([&id])?;
            let mut stats = GroupAnswers::default();
            while let Some(row) = rows.next()? {
                let Some((request, &timestamp)) = parents.get_key_value(row.get_ref(0)?.as_str()?)
                else {
                    continue;
                };
                let valid = row.get::<_, bool>(1)?;
                let (number, confidence, choice) = if valid {
                    (
                        row.get(2)?,
                        row.get(3)?,
                        if kind == "choice" {
                            Some(row.get_ref(4)?.as_str()?)
                        } else {
                            None
                        },
                    )
                } else {
                    (None, None, None)
                };
                stats.add(
                    &kind,
                    request,
                    timestamp,
                    AnswerValues {
                        valid,
                        number,
                        confidence,
                        choice,
                    },
                )?;
            }
            // Retain one page plus lookahead, even for high-cardinality data.
            retain_dashboard_group(&mut selected, id, stats, &kind, &after);
        }
        Ok(selected)
    }

    fn group_cursor(filter: &Filter) -> Result<Option<(i64, String)>> {
        filter
            .group_cursor
            .as_deref()
            .map(|cursor| {
                let (timestamp, id) = cursor.split_once(':').context("Invalid group cursor")?;
                if id.is_empty() {
                    bail!("Invalid group cursor");
                }
                Ok((
                    timestamp.parse::<i64>().context("Invalid group cursor")?,
                    id.to_owned(),
                ))
            })
            .transpose()
    }

    pub fn dashboard(&self, filter: &Filter) -> Result<Value> {
        let mut anchored = filter.clone();
        anchored
            .as_of
            .get_or_insert_with(|| Utc::now().timestamp_millis());
        let mut conn = self.reader()?;
        Self::dashboard_from(&mut conn, &anchored)
    }

    pub(super) fn dashboard_from(conn: &mut Connection, filter: &Filter) -> Result<Value> {
        let tx = conn.transaction()?;
        let (predicate, args) = Self::dashboard_predicate(&tx, filter)?;
        let DashboardRequests {
            sample,
            summary,
            timeline,
            parents,
            timeline_meta,
        } = Self::dashboard_requests(&tx, &predicate, &args, filter)?;
        // The overview already proved whether any parent matches. Keep cursor
        // validation in request_page, but avoid another history scan when empty.
        let (feed_predicate, feed_args) = if parents.is_empty() {
            ("0", &[][..])
        } else {
            (predicate.as_str(), args.as_slice())
        };
        let (requests, feed_next_cursor) = Self::request_page(
            &tx,
            feed_predicate,
            feed_args,
            filter.request_cursor.as_deref(),
        )?;
        // Families may contain multiple keys per request. Keep their exact SQL
        // distinct counts; strict groups compute metrics and bins in one pass.
        let from = format!(
            "FROM groups g INDEXED BY sqlite_autoindex_groups_1 CROSS JOIN answers a INDEXED BY answer_statistics ON a.group_id=g.id CROSS JOIN requests r INDEXED BY request_join_scope ON r.id=a.request_id WHERE {predicate}"
        );
        let metrics = "count(*) AS answer_count,coalesce(sum(a.valid),0) AS valid_count,count(DISTINCT r.id) AS request_count,max(r.timestamp) AS last_seen,avg(CASE WHEN a.valid THEN a.value_num END) AS mean_value,avg(CASE WHEN a.valid THEN a.confidence END) AS mean_confidence";
        let mut stats = Self::strict_dashboard_groups(&tx, &parents, filter)?;
        let mut family_args = args.clone();
        let mut family_scope = String::new();
        if let Some(search) = filter
            .group_search
            .as_ref()
            .filter(|search| !search.is_empty())
        {
            family_scope.push_str(" AND instr(lower(coalesce(g.family_name,'') || ' ' || g.source || ' ' || g.kind || ' ' || g.family_id),lower(?))>0");
            family_args.push(search.clone().into());
        }
        let mut family_after = String::new();
        if let Some((timestamp, id)) = Self::group_cursor(filter)? {
            family_after.push_str(" HAVING last_seen<? OR (last_seen=? AND display_id>?)");
            family_args.extend([timestamp.into(), timestamp.into(), id.into()]);
        }
        let mut stmt = tx.prepare(&format!("SELECT g.family_id AS display_id,{metrics} {from} AND g.family_id IS NOT NULL {family_scope} GROUP BY g.family_id {family_after} ORDER BY last_seen DESC,display_id LIMIT 101"))?;
        let family_stats = stmt.query_map(params_from_iter(&family_args), |r| Ok((r.get::<_,String>(0)?, json!({
            "answer_count":r.get::<_,i64>(1)?,"valid_count":r.get::<_,i64>(2)?,"request_count":r.get::<_,i64>(3)?,
            "last_seen":r.get::<_,Option<i64>>(4)?,"mean_value":r.get::<_,Option<f64>>(5)?,"mean_confidence":r.get::<_,Option<f64>>(6)?,"distribution":[]
        }))))?.collect::<rusqlite::Result<Vec<_>>>()?;
        stats.extend(family_stats);
        stats.sort_by(|(left_id, left), (right_id, right)| {
            right["last_seen"]
                .as_i64()
                .cmp(&left["last_seen"].as_i64())
                .then_with(|| left_id.cmp(right_id))
        });
        let more_groups = stats.len() > 100;
        stats.truncate(100);
        let group_next_cursor = if more_groups {
            stats
                .last()
                .map(|(id, stats)| format!("{}:{id}", stats["last_seen"].as_i64().unwrap()))
        } else {
            None
        };
        let mut groups = Vec::new();
        let mut version_counts = VersionCounts::new();
        let reuse_metadata = stats.len() > 1;
        for (id, stats) in &stats {
            if let Some(mut group) =
                Self::group_metadata_with_reuse(&tx, id, &mut version_counts, reuse_metadata)?
            {
                group
                    .as_object_mut()
                    .unwrap()
                    .extend(stats.as_object().unwrap().clone());
                let distribution: Vec<_> = group["distribution"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|bin| {
                        distribution_bin(
                            &group,
                            bin["label"].as_str().unwrap(),
                            bin["count"].clone(),
                        )
                    })
                    .collect();
                group["distribution"] = json!(distribution);
                // Comparison-only metrics belong to the detail endpoint. They
                // read saved answer JSON and must not run for every group on
                // each overview poll when the overview does not display them.
                groups.push(group);
            }
        }
        let families: Vec<_> = groups.iter().filter(|g| g["is_family"] == true).collect();
        if !families.is_empty() {
            let placeholders = vec!["?"; families.len()].join(",");
            let mut distribution_args = args.clone();
            distribution_args.extend(families.iter().map(|g| SqlValue::from(text(g, "id"))));
            let bin = "CASE a.kind WHEN 'choice' THEN a.value_text WHEN 'noul' THEN CAST(min(9,CAST(a.value_num*10 AS INTEGER)) AS TEXT) ELSE CAST(CAST(a.value_num AS INTEGER) AS TEXT) END";
            let mut stmt = tx.prepare(&format!("SELECT display_id,bin,bin_count FROM (SELECT g.family_id AS display_id,{bin} AS bin,count(*) AS bin_count,a.kind AS kind {from} AND a.valid=1 AND a.kind!='box' AND g.family_id IN ({placeholders}) GROUP BY g.family_id,bin) ORDER BY display_id,CASE WHEN kind='choice' THEN bin END,CASE WHEN kind<>'choice' THEN CAST(bin AS INTEGER) END"))?;
            let bins = stmt.query_map(params_from_iter(&distribution_args), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            let positions: std::collections::HashMap<String, usize> = groups
                .iter()
                .enumerate()
                .map(|(i, g)| (text(g, "id"), i))
                .collect();
            for bin in bins {
                let (id, raw, count) = bin?;
                if let Some(index) = positions.get(&id) {
                    let group = &mut groups[*index];
                    let bin = distribution_bin(group, &raw, json!(count));
                    group["distribution"].as_array_mut().unwrap().push(bin);
                }
            }
        }
        // Seek past duplicate runs in the existing source/model indexes. The
        // catalogs describe retained history and stay capped at 1000 values,
        // independently of the selected request scope.
        let mut sources = tx.prepare(
            "WITH RECURSIVE source_catalog(value) AS (
                SELECT min(source) FROM requests
                UNION ALL SELECT (SELECT min(source) FROM requests WHERE source>source_catalog.value)
                FROM source_catalog WHERE value IS NOT NULL LIMIT 1000
            ) SELECT value FROM source_catalog WHERE value IS NOT NULL ORDER BY value",
        )?;
        let sources = sources
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut models = tx.prepare(
            "WITH RECURSIVE model_catalog(value) AS (
                SELECT min(model) FROM requests WHERE model>''
                UNION ALL SELECT (SELECT min(model) FROM requests WHERE model>model_catalog.value)
                FROM model_catalog WHERE value IS NOT NULL LIMIT 1000
            ) SELECT value FROM model_catalog WHERE value IS NOT NULL ORDER BY value",
        )?;
        let models = models
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"generated_at":filter.as_of,"sample":sample,"summary":summary,"timeline":timeline,"timeline_meta":timeline_meta,"groups":groups,"requests":requests,"sources":sources,"models":models,"health":{},"feed_limit":100,"group_limit":100,"feed_next_cursor":feed_next_cursor,"group_next_cursor":group_next_cursor}),
        )
    }
}
