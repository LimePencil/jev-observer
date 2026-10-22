use super::{Filter, Store};
use anyhow::{Result, bail};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value as SqlValue};
use serde_json::{Value, json};

/// Export state moves between blocking tasks; no transaction spans a network send.
/// Only the selection connection may use disk-backed temporary storage. It
/// materializes integer sequences, never serialized parents or answers.
pub struct ExportSession {
    pub(super) store: Store,
    pub(super) selection: Option<Connection>,
    pub(super) predicate: String,
    pub(super) args: Vec<SqlValue>,
    pub(super) format: String,
    pub(super) after: i64,
    pub(super) through: i64,
}

impl Store {
    /// Freeze the rowid ceiling and any source/model selection once. Those
    /// indexes order by timestamp, so repeating an ORDER BY seq page query
    /// would scan and sort the same selected history for every page.
    pub fn start_export(&self, filter: &Filter, format: &str) -> Result<ExportSession> {
        self.start_export_from(self.reader()?, filter, format)
    }

    pub(super) fn start_export_from(
        &self,
        mut connection: Connection,
        filter: &Filter,
        format: &str,
    ) -> Result<ExportSession> {
        if !["jsonl", "csv"].contains(&format) {
            bail!("Export format must be jsonl or csv");
        }
        let mut filter = filter.clone();
        filter
            .as_of
            .get_or_insert_with(|| Utc::now().timestamp_millis());
        let (predicate, args) = Self::predicate(&filter)?;
        let frozen = [&filter.source, &filter.model]
            .into_iter()
            .flatten()
            .any(|value| !value.is_empty());
        if frozen {
            // Predicates may inspect a JSON scalar such as transport_error,
            // but only integer sequences enter temporary storage. Record
            // serialization uses a separate MEMORY connection on every page.
            connection.execute_batch(
                "PRAGMA temp_store=FILE; PRAGMA temp.cache_size=-2048; PRAGMA cache_size=-2048;",
            )?;
        }
        let transaction = connection.transaction()?;
        let through: i64 =
            transaction.query_row("SELECT coalesce(max(seq),0) FROM requests", [], |row| {
                row.get(0)
            })?;
        if frozen {
            transaction
                .execute_batch("CREATE TEMP TABLE export_request_ids(seq INTEGER PRIMARY KEY);")?;
            let mut selection_args = args.clone();
            selection_args.push(through.into());
            transaction.execute(
                &format!("INSERT INTO temp.export_request_ids SELECT r.seq FROM requests r WHERE {predicate} AND r.seq>0 AND r.seq<=?"),
                params_from_iter(&selection_args),
            )?;
        }
        transaction.commit()?;
        Ok(ExportSession {
            store: self.clone(),
            selection: frozen.then_some(connection),
            predicate,
            args,
            format: format.into(),
            after: 0,
            through,
        })
    }

    pub(super) fn export_page_from(
        conn: &rusqlite::Transaction<'_>,
        format: &str,
        after: i64,
        through: i64,
        ids: Vec<(Option<String>, i64)>,
        target_bytes: usize,
        max_bytes: usize,
    ) -> Result<(String, i64, i64, bool)> {
        let full_page = ids.len() == 100;
        let mut out = String::new();
        if format == "csv" && after == 0 {
            out.push_str("id,timestamp,source,model,status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,event_kind\n");
        }
        // The fixed CSV header is outside the record-data cap. Even a first
        // row exactly at the cap must advance the cursor, never emit only a
        // header and retry the same row. Total bytes remain capped plus header.
        let header_bytes = out.len();
        let mut cursor = after;
        for (id, seq) in ids {
            // A selected parent may disappear between pages. Advancing over
            // its immutable sequence also handles an entirely deleted page.
            let Some(id) = id else {
                cursor = seq;
                continue;
            };
            if out.len() - header_bytes >= target_bytes {
                return Ok((out, cursor, through, true));
            }
            let mut row = String::new();
            if format == "jsonl" {
                if let Some(record) = Self::request_from(conn, &id)? {
                    row = serde_json::to_string(&record)?;
                    row.push('\n');
                }
            } else if let Some((record, answer_count)) = Self::request_parent(conn, &id)? {
                // CSV contains parent metrics only. Its saved answer count is
                // committed atomically with the children, so exporting it does
                // not require loading every answer, definition and review.
                let fields = [
                    json!(id),
                    record["timestamp"].clone(),
                    record["source"].clone(),
                    record["model"].clone(),
                    record["status"].clone(),
                    record["duration_ms"].clone(),
                    record["input_tokens"].clone(),
                    record["output_tokens"].clone(),
                    record["cost_usd"].clone(),
                    record["cost_basis"].clone(),
                    json!(answer_count),
                    json!(record["event_kind"].as_str().unwrap_or("request")),
                ];
                row = fields.iter().map(csv_cell).collect::<Vec<_>>().join(",");
                row.push('\n');
            }
            if row.len() > max_bytes {
                bail!("An export record exceeds the 32 MiB page limit");
            }
            if out.len() - header_bytes > max_bytes - row.len() {
                // Keep this row for the next short read transaction. Only
                // successfully appended records may advance the cursor.
                return Ok((out, cursor, through, true));
            }
            out.push_str(&row);
            cursor = seq;
        }
        Ok((out, cursor, through, full_page))
    }

    #[cfg(test)]
    pub fn export(&self, filter: &Filter, format: &str) -> Result<String> {
        let mut session = self.start_export(filter, format)?;
        let mut output = String::new();
        loop {
            let (page, _, _, more) = session.next_page()?;
            output.push_str(&page);
            if output.len() > 16 * 1024 * 1024 {
                bail!("Use paginated exports for large histories");
            }
            if !more {
                break;
            }
        }
        Ok(output)
    }
}

impl ExportSession {
    pub fn next_page(&mut self) -> Result<(String, i64, i64, bool)> {
        self.next_page_with_limits(2 * 1024 * 1024, 32 * 1024 * 1024)
    }

    pub(super) fn next_page_with_limits(
        &mut self,
        target_bytes: usize,
        max_bytes: usize,
    ) -> Result<(String, i64, i64, bool)> {
        let selected = self.selection.as_ref().map(|selection| -> Result<Vec<i64>> {
            Ok(selection.prepare("SELECT seq FROM temp.export_request_ids WHERE seq>? AND seq<=? ORDER BY seq LIMIT 100")?
                .query_map(params![self.after, self.through], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        }).transpose()?;
        let mut connection = self.store.reader()?;
        // Parent/answer reads can sort original JSON (for example by answer
        // rowid). Keep those temporary values in memory, including demo mode.
        connection.execute_batch("PRAGMA temp_store=MEMORY;")?;
        let transaction = connection.transaction()?;
        let ids = if let Some(selected) = selected {
            let mut lookup = transaction.prepare("SELECT id FROM requests WHERE seq=?")?;
            selected
                .into_iter()
                .map(|seq| {
                    let id = lookup.query_row([seq], |row| row.get(0)).optional()?;
                    Ok((id, seq))
                })
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            let mut args = self.args.clone();
            args.push(self.after.into());
            args.push(self.through.into());
            transaction.prepare(&format!("SELECT id,seq FROM requests r WHERE {} AND seq>? AND seq<=? ORDER BY seq LIMIT 100", self.predicate))?
                .query_map(params_from_iter(&args), |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let page = Store::export_page_from(
            &transaction,
            &self.format,
            self.after,
            self.through,
            ids,
            target_bytes,
            max_bytes,
        )?;
        if page.3 && page.1 <= self.after {
            bail!("Export cursor did not advance");
        }
        transaction.commit()?;
        self.after = page.1;
        Ok(page)
    }
}

pub(super) fn csv_cell(value: &Value) -> String {
    let raw = match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => value.to_string(),
    };
    // Prevent spreadsheet formula interpretation without changing JSONL exports.
    let first_visible = raw.trim_start_matches(char::is_whitespace).chars().next();
    let formula = matches!(
        first_visible,
        Some('=' | '+' | '-' | '@' | '＝' | '＋' | '－' | '＠')
    );
    let leading_control = raw.starts_with(['\t', '\r', '\n', '\0']);
    let safe = if formula || leading_control {
        format!("'{raw}")
    } else {
        raw
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}
