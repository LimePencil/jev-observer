use super::{Filter, Store};
use anyhow::{Result, bail};
use chrono::Utc;
use rusqlite::{Connection, params_from_iter, types::Value as SqlValue};

pub(super) const DASHBOARD_SELECTION: &str =
    "r.seq IN (SELECT seq FROM temp.dashboard_request_ids)";
pub(super) const GROUP_SELECTION: &str = "r.seq IN (SELECT seq FROM temp.group_request_ids)";
pub(super) const GROUP_TIMELINE_SELECTION: &str = "r.timestamp BETWEEN (SELECT min(timestamp) FROM temp.group_request_ids) AND (SELECT max(timestamp) FROM temp.group_request_ids) AND r.seq IN (SELECT seq FROM temp.group_request_ids)";
pub(super) const FAILED: &str =
    "(r.status>=400 OR json_extract(r.data,'$.transport_error') IS NOT NULL)";

impl Store {
    pub(super) fn predicate(filter: &Filter) -> Result<(String, Vec<SqlValue>)> {
        Self::request_predicate(filter, false, None)
    }

    pub(super) fn request_predicate(
        filter: &Filter,
        batch_search: bool,
        answer_keys: Option<&[String]>,
    ) -> Result<(String, Vec<SqlValue>)> {
        let mut clauses = vec!["1=1".to_owned()];
        let mut args = Vec::new();
        let period = match filter.window.as_deref().unwrap_or("24h") {
            "1h" => Some(3_600_000),
            "24h" => Some(86_400_000),
            "7d" => Some(604_800_000),
            "all" => None,
            _ => bail!("Unknown time window"),
        };
        if let Some(period) = period.filter(|_| filter.from.is_none() && filter.to.is_none()) {
            clauses.push("r.timestamp>=? AND r.timestamp<=?".into());
            let as_of = filter
                .as_of
                .unwrap_or_else(|| Utc::now().timestamp_millis());
            args.push((as_of - period).into());
            args.push(as_of.into());
        }
        if let Some(from) = filter.from {
            clauses.push("r.timestamp>=?".into());
            args.push(from.into());
        }
        if let Some(to) = filter.to {
            clauses.push("r.timestamp<=?".into());
            args.push(to.into());
        }
        if filter
            .from
            .zip(filter.to)
            .is_some_and(|(from, to)| from > to)
        {
            bail!("Date range start must not follow its end");
        }
        for (name, value) in [("source", &filter.source), ("model", &filter.model)] {
            if let Some(value) = value.as_ref().filter(|s| !s.is_empty()) {
                clauses.push(format!("r.{name}=?"));
                args.push(value.clone().into());
            }
        }
        if filter.status.as_deref() == Some("error") {
            clauses.push(FAILED.into());
        }
        if let Some(group) = filter.group.as_ref().filter(|group| !group.is_empty()) {
            clauses.push("EXISTS(SELECT 1 FROM answers ga JOIN groups gg ON gg.id=ga.group_id WHERE ga.request_id=r.id AND (gg.id=? OR gg.family_id=?))".into());
            args.push(group.clone().into());
            args.push(group.clone().into());
        }
        if let Some(search) = filter.search.as_ref().filter(|s| !s.is_empty()) {
            args.extend([search.clone().into(), search.clone().into()]);
            let answer_match = if let Some(keys) = answer_keys {
                if keys.is_empty() {
                    "0".to_owned()
                } else {
                    args.extend(keys.iter().cloned().map(SqlValue::from));
                    let placeholders = vec!["?"; keys.len()].join(",");
                    format!(
                        "r.id IN (SELECT sa.request_id FROM answers sa INDEXED BY answer_key WHERE sa.key IN ({placeholders}))"
                    )
                }
            } else {
                args.push(search.clone().into());
                if batch_search {
                    "r.id IN (SELECT sa.request_id FROM answers sa WHERE instr(lower(sa.key),lower(?))>0)".to_owned()
                } else {
                    "EXISTS(SELECT 1 FROM answers sa WHERE sa.request_id=r.id AND instr(lower(sa.key),lower(?))>0)".to_owned()
                }
            };
            clauses.push(format!("(instr(lower(r.source),lower(?))>0 OR instr(lower(r.id),lower(?))>0 OR {answer_match})"));
        }
        Ok((clauses.join(" AND "), args))
    }

    /// Seek over repeated keys, evaluating SQLite's original substring match
    /// once per distinct key. Bound discovery before filtering: high-cardinality
    /// histories fall back to the original scan rather than unbounded seeks.
    pub(super) fn dashboard_answer_keys(
        conn: &Connection,
        search: &str,
    ) -> Result<Option<Vec<String>>> {
        let mut stmt = conn.prepare(
            "WITH RECURSIVE keys(key) AS (
               SELECT (SELECT key FROM answers INDEXED BY answer_key ORDER BY key LIMIT 1)
               UNION ALL
               SELECT (SELECT key FROM answers INDEXED BY answer_key WHERE key>keys.key ORDER BY key LIMIT 1)
               FROM keys WHERE key IS NOT NULL LIMIT 513
             ) SELECT key,instr(lower(key),lower(?))>0 FROM keys WHERE key IS NOT NULL",
        )?;
        let mut rows = stmt.query([search])?;
        let mut matched = Vec::new();
        let mut matched_bytes = 0;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            count += 1;
            if count > 512 {
                return Ok(None);
            }
            if row.get::<_, bool>(1)? {
                let key = row.get_ref(0)?.as_str()?;
                // Imported keys can be large. Bound copied bind values as well
                // as key count; fallback keeps the original search semantics.
                if key.len() > 128 * 1024 - matched_bytes {
                    return Ok(None);
                }
                matched_bytes += key.len();
                matched.push(key.to_owned());
            }
        }
        Ok(Some(matched))
    }

    /// Expensive answer-key/group filters select parent requests once per
    /// snapshot. Reuse their integer row IDs for every aggregate and feed query.
    pub(super) fn dashboard_predicate(
        conn: &Connection,
        filter: &Filter,
    ) -> Result<(String, Vec<SqlValue>)> {
        // Broad searches resolve answer-key matches in one covering scan;
        // narrow scopes keep per-parent lookups rather than scanning history.
        // Export/detail queries keep their existing paged/correlated strategy.
        let mut batch_search = false;
        let mut narrow_date_scope = false;
        let mut retained = None;
        let has_group = filter.group.as_ref().is_some_and(|group| !group.is_empty());
        if !has_group && filter.search.as_ref().is_some_and(|s| !s.is_empty()) {
            let mut scope = filter.clone();
            scope.search = None;
            let (predicate, args) = Self::predicate(&scope)?;
            let records: i64 =
                conn.query_row("SELECT count(*) FROM requests", [], |row| row.get(0))?;
            retained = Some(records);
            let threshold = (records / 4).max(512);
            batch_search = conn.query_row(
                &format!("SELECT count(*)>{threshold} FROM (SELECT 1 FROM requests r WHERE {predicate} LIMIT {})", threshold + 1),
                params_from_iter(&args), |row| row.get(0),
            )?;
            // Reuse this bounded count only when dates alone made the parent
            // scope narrow. A rare source/model/error filter does not prove
            // that walking the timestamp range will avoid unrelated history.
            narrow_date_scope = !batch_search
                && (filter.from.is_some() || filter.to.is_some())
                && filter.source.as_ref().is_none_or(|value| value.is_empty())
                && filter.model.as_ref().is_none_or(|value| value.is_empty())
                && filter.status.as_deref() != Some("error");
        }
        let answer_keys = if batch_search {
            Self::dashboard_answer_keys(conn, filter.search.as_deref().unwrap_or(""))?
        } else {
            None
        };
        let (predicate, args) =
            Self::request_predicate(filter, batch_search, answer_keys.as_deref())?;
        if !has_group
            && filter
                .search
                .as_ref()
                .is_none_or(|search| search.is_empty())
        {
            return Ok((predicate, args));
        }
        // Keep the integer selection in the reader's MEMORY temp store.
        // Later queries on this same snapshot can sort retained text, so
        // switching the connection to FILE here would also spill payloads.
        conn.execute_batch("CREATE TEMP TABLE dashboard_request_ids(seq INTEGER PRIMARY KEY);")?;
        // All predicate fields live in these indexes. Reading the request body
        // table here would pull large saved JSON pages into every search poll.
        let index = if filter.window.as_deref() == Some("all") && !narrow_date_scope {
            "request_join_scope"
        } else {
            "request_dashboard"
        };
        let selected = conn.execute(
            &format!("INSERT INTO temp.dashboard_request_ids SELECT r.seq FROM requests r INDEXED BY {index} WHERE {predicate}"),
            params_from_iter(&args),
        )?;
        // Both counts are exact in this read transaction. When every retained
        // parent matched, later queries need no temporary membership lookup.
        if retained.is_some_and(|records| usize::try_from(records).ok() == Some(selected)) {
            return Ok(("1=1".into(), Vec::new()));
        }
        Ok((DASHBOARD_SELECTION.into(), Vec::new()))
    }
}
