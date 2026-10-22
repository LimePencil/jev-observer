use super::{Filter, Store, filter::FAILED};
use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value as SqlValue};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Mean {
    sum: f64,
    correction: f64,
    pub(super) count: u64,
}

impl Mean {
    pub(super) fn add(&mut self, value: f64) {
        // Compensated summation matches SQLite AVG's treatment of REAL values.
        let next = self.sum + value;
        self.correction += if self.sum.abs() > value.abs() {
            (self.sum - next) + value
        } else {
            (value - next) + self.sum
        };
        self.sum = next;
        self.count += 1;
    }

    pub(super) fn total(&self) -> Option<f64> {
        (self.count > 0).then_some(self.sum + self.correction)
    }

    pub(super) fn value(&self) -> Option<f64> {
        self.total().map(|sum| sum / self.count as f64)
    }
}

#[derive(Default)]
pub(super) struct TimelineBucket {
    pub(super) requests: i64,
    pub(super) errors: i64,
    pub(super) latency: Mean,
    pub(super) cost: Mean,
}

pub(super) struct TimelineSpec {
    pub(super) start: i64,
    pub(super) end: i64,
    pub(super) width: i64,
    pub(super) count: usize,
}

impl TimelineSpec {
    pub(super) fn bucket(&self, timestamp: i64) -> i64 {
        let offset = (i128::from(timestamp) - i128::from(self.start)) / i128::from(self.width);
        (i128::from(self.start) + offset * i128::from(self.width)) as i64
    }

    pub(super) fn metadata(&self) -> Value {
        json!({"start":self.start,"end":self.end.saturating_add(1),"bucket_width":self.width,"truncated":false})
    }

    pub(super) fn finish(&self, mut buckets: BTreeMap<i64, TimelineBucket>) -> Vec<Value> {
        (0..self.count)
            .map(|index| {
                let timestamp =
                    (i128::from(self.start) + index as i128 * i128::from(self.width)) as i64;
                let bucket = buckets.remove(&timestamp).unwrap_or_default();
                json!({"timestamp":timestamp,"requests":bucket.requests,"errors":bucket.errors,
                "mean_latency_ms":bucket.latency.value(),"cost_usd":bucket.cost.total()})
            })
            .collect()
    }
}

/// Token counts are nonnegative signed 64-bit integers. Keep ordinary totals
/// exact, and distinguish an unrepresentable sum from missing reported usage.
#[derive(Default)]
pub(super) struct TokenSum {
    pub(super) total: Option<i64>,
    pub(super) known: i64,
    pub(super) overflow: bool,
}

impl TokenSum {
    pub(super) fn add(&mut self, value: Option<i64>) {
        if let Some(value) = value {
            self.known += 1;
            if !self.overflow {
                self.total = self.total.unwrap_or(0).checked_add(value);
                self.overflow = self.total.is_none();
            }
        }
    }
}

impl Store {
    pub(super) fn timeline_spec(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        filter: &Filter,
    ) -> Result<TimelineSpec> {
        let now = filter
            .as_of
            .unwrap_or_else(|| Utc::now().timestamp_millis());
        let custom = filter.from.is_some() || filter.to.is_some();
        let period = match filter.window.as_deref().unwrap_or("24h") {
            "1h" => Some(3_600_000_i64),
            "24h" => Some(86_400_000),
            "7d" => Some(604_800_000),
            _ => None,
        };
        let (start, end) = if let Some(period) = period.filter(|_| !custom) {
            (now.saturating_sub(period), now)
        } else if let (Some(start), Some(end)) = (filter.from, filter.to) {
            // Explicit limits already determine both bounds; history cannot change them.
            (start, end.max(start))
        } else {
            // Probe a bounded number of oldest/newest parents before falling
            // back to the covering aggregate. Dense histories usually resolve
            // both bounds without scanning every retained request. The limit
            // applies before filtering, so sparse/empty scopes add bounded work.
            let edge_predicate = predicate.replace(
                "json_extract(r.data,'$.transport_error')",
                "r.transport_error",
            );
            let probe = |direction: &str| -> Result<Option<i64>> {
                Ok(conn.query_row(
                    &format!("SELECT r.timestamp FROM (SELECT seq,id,timestamp,source,model,status,event_kind,json_extract(data,'$.timestamp') AS original_time,json_extract(data,'$.transport_error') AS transport_error FROM requests INDEXED BY request_dashboard ORDER BY timestamp {direction} LIMIT 512) r WHERE {edge_predicate} AND r.event_kind='request' AND r.original_time IS NOT NULL ORDER BY r.timestamp {direction} LIMIT 1"),
                    params_from_iter(args), |row| row.get(0),
                ).optional()?)
            };
            let first = if filter.from.is_none() {
                probe("ASC")?
            } else {
                None
            };
            let last = if filter.to.is_none() {
                probe("DESC")?
            } else {
                None
            };
            let (first, last) = if (filter.from.is_none() && first.is_none())
                || (filter.to.is_none() && last.is_none())
            {
                conn.query_row(
                    &format!("SELECT min(timestamp),max(timestamp) FROM requests r INDEXED BY request_dashboard WHERE {predicate} AND event_kind='request' AND json_extract(data,'$.timestamp') IS NOT NULL"),
                    params_from_iter(args), |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
                )?
            } else {
                (first, last)
            };
            let end = filter.to.or(last).unwrap_or(now);
            let start = filter
                .from
                .or(first)
                .unwrap_or_else(|| end.saturating_sub(86_400_000));
            (start, end.max(start))
        };
        let preferred = match (custom, filter.window.as_deref().unwrap_or("24h")) {
            (false, "24h") => 1_800_000_i128,
            (false, "7d") => 3_600_000,
            _ => 60_000,
        };
        let span = i128::from(end) - i128::from(start) + 1;
        let required = (span + 167) / 168;
        let width = (((required.max(preferred) + preferred - 1) / preferred) * preferred)
            .min(i128::from(i64::MAX)) as i64;
        let count = ((span + i128::from(width) - 1) / i128::from(width)) as usize;
        Ok(TimelineSpec {
            start,
            end,
            width,
            count,
        })
    }

    pub(super) fn timeline(
        conn: &Connection,
        predicate: &str,
        args: &[SqlValue],
        filter: &Filter,
    ) -> Result<(Vec<Value>, Value)> {
        let spec = Self::timeline_spec(conn, predicate, args, filter)?;
        let mut buckets = BTreeMap::<i64, TimelineBucket>::new();
        let mut stmt = conn.prepare(&format!(
            "SELECT timestamp,{FAILED},duration_ms,cost_usd FROM requests r INDEXED BY request_dashboard WHERE {predicate} AND event_kind='request' AND json_extract(data,'$.timestamp') IS NOT NULL"
        ))?;
        let mut rows = stmt.query(params_from_iter(args))?;
        while let Some(row) = rows.next()? {
            let bucket = buckets.entry(spec.bucket(row.get(0)?)).or_default();
            bucket.requests += 1;
            bucket.errors += i64::from(row.get::<_, Option<bool>>(1)?.unwrap_or(false));
            if let Some(value) = row.get::<_, Option<f64>>(2)? {
                bucket.latency.add(value);
            }
            if let Some(value) = row.get::<_, Option<f64>>(3)? {
                bucket.cost.add(value);
            }
        }
        Ok((spec.finish(buckets), spec.metadata()))
    }
}
