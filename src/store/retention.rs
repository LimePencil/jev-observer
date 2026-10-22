use super::Store;
use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension};

const DELETE_BATCH_SIZE: i64 = 256;

impl Store {
    /// Run age/count retention without requiring a new captured event.
    pub fn maintenance(&self, conn: &mut Connection, force: bool) -> Result<bool> {
        let tx = conn.transaction()?;
        let ran = self.prune_in_transaction(&tx, force)?;
        tx.commit()?;
        Ok(ran)
    }

    fn delete_request_batches(tx: &Connection, query: &str, cutoff: i64) -> Result<usize> {
        let mut statement = tx.prepare_cached(query)?;
        let mut removed = 0;
        loop {
            let changed = statement.execute([cutoff, DELETE_BATCH_SIZE])?;
            removed += changed;
            if changed < DELETE_BATCH_SIZE as usize {
                return Ok(removed);
            }
        }
    }

    fn delete_group_batches(tx: &Connection, orphaned_only: bool) -> Result<()> {
        // Cursor by rowid, including negative values and legacy NULL group IDs.
        // Advancing past each batch avoids rescanning a surviving group prefix.
        // Only integer boundaries are selected; definitions remain in the DB.
        let predicate = if orphaned_only {
            "NOT EXISTS(SELECT 1 FROM answers a WHERE a.group_id=groups.id)"
        } else {
            "1"
        };
        let mut first = tx.prepare_cached(&format!("SELECT min(seq),max(seq) FROM (SELECT rowid AS seq FROM groups WHERE {predicate} ORDER BY rowid LIMIT ?)"))?;
        let mut next = tx.prepare_cached(&format!("SELECT min(seq),max(seq) FROM (SELECT rowid AS seq FROM groups WHERE rowid>? AND {predicate} ORDER BY rowid LIMIT ?)"))?;
        let mut delete = tx.prepare_cached(&format!(
            "DELETE FROM groups WHERE rowid BETWEEN ? AND ? AND {predicate}"
        ))?;
        let bounds = |row: &rusqlite::Row<'_>| {
            Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?))
        };
        let mut after = None;
        loop {
            let range = match after {
                Some(after) => next.query_row([after, DELETE_BATCH_SIZE], bounds)?,
                None => first.query_row([DELETE_BATCH_SIZE], bounds)?,
            };
            let (Some(low), Some(high)) = range else {
                return Ok(());
            };
            delete.execute([low, high])?;
            after = Some(high);
        }
    }

    pub(super) fn prune_in_transaction(&self, tx: &Connection, force: bool) -> Result<bool> {
        let now = Utc::now().timestamp_millis();
        let last: i64 = tx
            .query_row(
                "SELECT value FROM maintenance WHERE key='pruned_at'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if force || now < last || now - last >= 30_000 {
            // Expression indexes and foreign keys can require a RAM statement
            // journal. Finish bounded DELETE statements while retaining the
            // existing outer transaction and fixed cutoffs for atomicity.
            let expired = Self::delete_request_batches(
                tx,
                "DELETE FROM requests WHERE seq IN (SELECT seq FROM requests WHERE timestamp < ? ORDER BY timestamp,seq LIMIT ?)",
                now - i64::from(self.retention_days) * 86_400_000,
            )?;
            // Distinct positive row IDs cannot outnumber their inclusive span.
            // Two endpoint lookups avoid walking the whole retained history
            // when it is provably within the cap. Gaps only widen this bound;
            // larger spans and legacy nonpositive IDs keep the exact query.
            let within_cap = match tx
                .query_row("SELECT seq FROM requests ORDER BY seq LIMIT 1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .optional()?
            {
                None => true,
                Some(first) if first > 0 => {
                    let last: i64 = tx.query_row(
                        "SELECT seq FROM requests ORDER BY seq DESC LIMIT 1",
                        [],
                        |row| row.get(0),
                    )?;
                    last.checked_sub(first)
                        .and_then(|span| span.checked_add(1))
                        .is_some_and(|span| span <= self.max_records as i64)
                }
                Some(_) => false,
            };
            let excess = if within_cap {
                0
            } else if let Ok(keep) = i64::try_from(self.max_records) {
                // Counting the narrow time index avoids visiting saved bodies.
                // Then walk whichever end has fewer ordinal entries: a small
                // newly appended excess, or the records kept after a large cut.
                let retained: i64 = tx.query_row(
                    "SELECT count(*) FROM requests INDEXED BY request_time",
                    [],
                    |row| row.get(0),
                )?;
                let cutoff = if retained > keep {
                    let removed = retained - keep;
                    let (query, offset) = if removed <= keep {
                        (
                            "SELECT seq FROM requests ORDER BY seq LIMIT 1 OFFSET ?",
                            removed - 1,
                        )
                    } else {
                        (
                            "SELECT seq FROM requests ORDER BY seq DESC LIMIT 1 OFFSET ?",
                            keep,
                        )
                    };
                    tx.query_row(query, [offset], |row| row.get::<_, i64>(0))?
                } else {
                    // Preserve the existing cutoff for legacy nonpositive IDs.
                    -1
                };
                Self::delete_request_batches(
                    tx,
                    "DELETE FROM requests WHERE seq IN (SELECT seq FROM requests WHERE seq <= ? ORDER BY seq LIMIT ?)",
                    cutoff,
                )?
            } else {
                let cutoff = tx.query_row("SELECT COALESCE((SELECT seq FROM requests ORDER BY seq DESC LIMIT 1 OFFSET ?),-1)", [self.max_records as i64], |row| row.get(0))?;
                Self::delete_request_batches(
                    tx,
                    "DELETE FROM requests WHERE seq IN (SELECT seq FROM requests WHERE seq <= ? ORDER BY seq LIMIT ?)",
                    cutoff,
                )?
            };
            // Parent/answer/group writes and cascading deletions are atomic.
            // A periodic pass that removed no parents cannot orphan a group.
            // Forced startup maintenance still repairs pre-existing orphans.
            if force || expired > 0 || excess > 0 {
                Self::delete_group_batches(tx, true)?;
            }
            tx.execute(
                "INSERT OR REPLACE INTO maintenance(key,value) VALUES('pruned_at',?)",
                [now],
            )?;
            return Ok(true);
        }
        Ok(false)
    }

    pub fn delete_all(&self) -> Result<()> {
        let mut conn = self.writer_connection()?;
        Self::delete_all_from_connection(&mut conn)
    }

    pub(super) fn delete_all_from_connection(conn: &mut Connection) -> Result<()> {
        let tx = conn.transaction()?;
        Self::delete_request_batches(
            &tx,
            "DELETE FROM requests WHERE seq IN (SELECT seq FROM requests WHERE seq <= ? ORDER BY seq LIMIT ?)",
            i64::MAX,
        )?;
        // Delete every group, retaining the original error behavior if corrupt
        // legacy history contains an answer whose missing parent cannot cascade.
        Self::delete_group_batches(&tx, false)?;
        tx.commit()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}
