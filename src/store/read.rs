#[cfg(test)]
use super::tests;
use super::{
    Store,
    filter::{DASHBOARD_SELECTION, FAILED},
    value::{label_fields, parse},
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value as SqlValue};
use serde_json::{Value, json};
use std::collections::HashMap;

impl Store {
    pub fn is_empty(&self) -> Result<bool> {
        Ok(self
            .reader()?
            .query_row("SELECT NOT EXISTS(SELECT 1 FROM requests)", [], |row| {
                row.get(0)
            })?)
    }

    pub(super) fn request_page(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        cursor: Option<&str>,
    ) -> Result<(Vec<Value>, Option<String>)> {
        let mut rows = Self::request_summaries_raw(conn, predicate, args, 101, cursor)?;
        let more = rows.len() > 100;
        rows.truncate(100);
        let next = if more {
            rows.last()
                .and_then(|row| row["_cursor"].as_str())
                .map(str::to_owned)
        } else {
            None
        };
        for row in &mut rows {
            row.as_object_mut().unwrap().remove("_cursor");
        }
        Ok((rows, next))
    }

    pub(super) fn request_summaries(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        limit: usize,
    ) -> Result<Vec<Value>> {
        let mut rows = Self::request_summaries_raw(conn, predicate, args, limit, None)?;
        for row in &mut rows {
            row.as_object_mut().unwrap().remove("_cursor");
        }
        Ok(rows)
    }

    pub(super) fn request_summary_query(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<(String, Vec<SqlValue>)> {
        // Dense selections can stop after a short newest-first index walk.
        // Sparse selections keep direct rowid lookups: even a page with only
        // two matches left could otherwise cross a large gap in the history.
        // These counts read only integer IDs and the narrow timestamp index.
        let broad_selection = if predicate == DASHBOARD_SELECTION {
            let selected: i64 = conn.query_row(
                "SELECT count(*) FROM temp.dashboard_request_ids",
                [],
                |row| row.get(0),
            )?;
            if selected > limit as i64 {
                let retained: i64 = conn.query_row(
                    "SELECT count(*) FROM requests INDEXED BY request_time",
                    [],
                    |row| row.get(0),
                )?;
                selected >= retained / 4
            } else {
                false
            }
        } else {
            false
        };
        let scan = if broad_selection {
            "requests r INDEXED BY request_time"
        } else {
            "requests r"
        };
        // Choose the scan from the parent selection before a cursor changes
        // the predicate. Later pages need the same bounded index walk.
        let mut scope = predicate.to_owned();
        let mut values = args.to_vec();
        if let Some(cursor) = cursor {
            let (timestamp, seq) = cursor.split_once(':').context("Invalid request cursor")?;
            let timestamp = timestamp.parse::<i64>().context("Invalid request cursor")?;
            let seq = seq.parse::<i64>().context("Invalid request cursor")?;
            scope.push_str(" AND (r.timestamp,r.seq)<(?,?)");
            values.extend([timestamp.into(), seq.into()]);
        }
        Ok((
            format!(
                "SELECT id,json_extract(data,'$.timestamp'),source,NULLIF(model,''),status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,capture_complete,sample,event_kind,json_extract(data,'$.imported_at'),json_extract(data,'$.transport_error'),coalesce({FAILED},0),timestamp,seq FROM {scan} WHERE {scope} ORDER BY timestamp DESC,seq DESC LIMIT {limit}"
            ),
            values,
        ))
    }

    pub(super) fn request_summaries_raw(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<Vec<Value>> {
        let (query, values) = Self::request_summary_query(conn, predicate, args, limit, cursor)?;
        let mut stmt = conn.prepare(&query)?;
        Ok(stmt.query_map(params_from_iter(&values), |r| Ok(json!({"id":r.get::<_,String>(0)?,"timestamp":r.get::<_,Option<i64>>(1)?,"source":r.get::<_,String>(2)?,"model":r.get::<_,Option<String>>(3)?,"status":r.get::<_,Option<i64>>(4)?,"duration_ms":r.get::<_,Option<f64>>(5)?,"input_tokens":r.get::<_,Option<i64>>(6)?,"output_tokens":r.get::<_,Option<i64>>(7)?,"cost_usd":r.get::<_,Option<f64>>(8)?,"cost_basis":r.get::<_,String>(9)?,"answer_count":r.get::<_,i64>(10)?,"capture_complete":r.get::<_,bool>(11)?,"sample":r.get::<_,bool>(12)?,"event_kind":r.get::<_,String>(13)?,"imported_at":r.get::<_,Option<i64>>(14)?,"transport_error":r.get::<_,Option<String>>(15)?,"failed":r.get::<_,bool>(16)?,"_cursor":format!("{}:{}",r.get::<_,i64>(17)?,r.get::<_,i64>(18)?)})))?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // A record spans parent, answers and labels. Requiring a transaction keeps
    // retention/deletion or review edits from mixing different read snapshots.
    pub(super) fn request_parent(
        conn: &rusqlite::Transaction<'_>,
        id: &str,
    ) -> Result<Option<(Value, i64)>> {
        let record = conn
            .query_row(
                "SELECT data,answer_count FROM requests WHERE id=?",
                [id],
                |r| Ok((parse(r.get(0)?)?, r.get(1)?)),
            )
            .optional()?;
        #[cfg(test)]
        if record.is_some() {
            tests::after_parent_read(id);
        }
        Ok(record)
    }

    pub(super) fn request_from(
        conn: &rusqlite::Transaction<'_>,
        id: &str,
    ) -> Result<Option<Value>> {
        let Some((mut record, _)) = Self::request_parent(conn, id)? else {
            return Ok(None);
        };
        let mut stmt=conn.prepare("SELECT a.data,g.definition FROM answers a JOIN groups g ON g.id=a.group_id WHERE request_id=? ORDER BY a.rowid")?;
        let answers = stmt
            .query_map([id], |r| {
                let mut value = parse(r.get(0)?)?;
                value["definition"] = parse(r.get(1)?)?;
                Ok(value)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        record["answers"] = Value::Array(answers);
        // Legacy databases kept the original annotations only in the parent.
        // Last valid annotation wins, matching write_batch's replacement order.
        let imported_labels: HashMap<_, _> = record["labels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|annotation| label_fields(annotation).map(|(key, _)| (key, annotation)))
            .collect();
        let mut stmt = conn.prepare(
            "SELECT key,label,timestamp,source,annotation FROM labels WHERE request_id=? ORDER BY key",
        )?;
        let labels = stmt
            .query_map([id], |row| {
                let key: String = row.get(0)?;
                let label: String = row.get(1)?;
                let timestamp: i64 = row.get(2)?;
                let source: String = row.get(3)?;
                let saved: Option<String> = row.get(4)?;
                let mut annotation = if let Some(saved) = saved {
                    parse(saved)?
                } else {
                    // A local replacement must never inherit imported notes,
                    // even if its value and millisecond timestamp happen to match.
                    imported_labels
                        .get(key.as_str())
                        .filter(|original| {
                            source != "local-reviewer"
                                && original["label"].as_str() == Some(label.as_str())
                                && original["timestamp"].as_i64() == Some(timestamp)
                                && original["source"].as_str().unwrap_or("import") == source
                        })
                        .map(|original| (*original).clone())
                        .unwrap_or_else(|| json!({}))
                };
                if !annotation.is_object() {
                    annotation = json!({});
                }
                // The indexed columns remain authoritative for current review state.
                annotation["key"] = json!(key);
                annotation["label"] = json!(label);
                annotation["timestamp"] = json!(timestamp);
                annotation["source"] = json!(source);
                Ok(annotation)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        record["labels"] = Value::Array(labels);
        record["failed"] = json!(
            record["status"]
                .as_i64()
                .is_some_and(|status| status >= 400)
                || !record["transport_error"].is_null()
        );
        Ok(Some(record))
    }

    pub fn request(&self, id: &str) -> Result<Option<Value>> {
        let mut conn = self.reader()?;
        let tx = conn.transaction()?;
        Self::request_from(&tx, id)
    }
}
