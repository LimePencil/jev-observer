use super::{
    LabelOutcome, Store,
    value::{label_fields, optional_text, serialize_without_field, text},
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

impl Store {
    /// Only a digest of a random local token is stored here. It prevents an
    /// obsolete system credential from becoming active after a locked store
    /// prevented its immediate deletion.
    pub fn credential_approval(&self) -> Result<Option<String>> {
        Ok(self
            .reader()?
            .query_row(
                "SELECT token_hash FROM credential_approval WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn set_credential_approval(&self, token_hash: Option<&str>) -> Result<()> {
        let connection = self.writer_connection()?;
        if let Some(token_hash) = token_hash {
            connection.execute(
                "INSERT INTO credential_approval(id,token_hash) VALUES(1,?) \
                 ON CONFLICT(id) DO UPDATE SET token_hash=excluded.token_hash",
                [token_hash],
            )?;
        } else {
            connection.execute("DELETE FROM credential_approval WHERE id=1", [])?;
        }
        Ok(())
    }

    pub fn write_batch(&self, conn: &mut Connection, records: &[Value]) -> Result<usize> {
        let tx = conn.transaction()?;
        let mut inserted = 0;
        for record in records {
            let id = text(record, "id");
            if id.is_empty() {
                bail!("Record ID is required");
            }
            let parent = record.as_object().context("Record must be an object")?;
            let answers = record["answers"]
                .as_array()
                .context("Record answers must be an array")?;
            let count = tx.prepare_cached("INSERT OR IGNORE INTO requests(id,timestamp,source,model,status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,capture_complete,sample,source_event_id,import_format,event_kind,data) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")?.execute(
                params![id, record["timestamp"].as_i64().or(record["imported_at"].as_i64()).context("timestamp or imported_at is required")?, text(record,"source"),text(record,"model"),record["status"].as_i64(),record["duration_ms"].as_f64(),record["input_tokens"].as_i64(),record["output_tokens"].as_i64(),record["cost_usd"].as_f64(),text(record,"cost_basis"),answers.len() as i64,record["capture_complete"].as_bool().unwrap_or(false),record["sample"].as_bool().unwrap_or(false),optional_text(record,"source_event_id"),optional_text(record,"import_format"),record["event_kind"].as_str().unwrap_or("request"),serialize_without_field(parent,"answers")?])?;
            if count == 0 {
                continue;
            }
            inserted += 1;
            for answer in answers {
                tx.prepare_cached("INSERT OR IGNORE INTO groups(id,key,kind,source,definition_id,presentation_id,definition,task_version,family_id,family_name,adapter) VALUES(?,?,?,?,?,?,?,?,?,?,?)")?.execute( params![text(answer,"group_id"),text(answer,"key"),text(answer,"kind"),text(record,"source"),text(answer,"definition_id"),text(answer,"presentation_id"),serde_json::to_string(&answer["definition"])?,optional_text(answer,"task_version"),optional_text(answer,"family_id"),optional_text(answer,"family_name"),optional_text(answer,"adapter")])?;
                let child = answer.as_object().context("Answer must be an object")?;
                tx.prepare_cached("INSERT INTO answers(request_id,key,group_id,kind,valid,value_num,value_text,confidence,data) VALUES(?,?,?,?,?,?,?,?,?)")?.execute(params![id,text(answer,"key"),text(answer,"group_id"),text(answer,"kind"),answer["valid"].as_bool().unwrap_or(false),answer["value"].as_f64(),answer["value"].as_str(),answer["confidence"].as_f64(),serialize_without_field(child,"definition")?])?;
            }
            if let Some(labels) = record["labels"].as_array() {
                for label in labels {
                    if let Some((key, value)) = label_fields(label) {
                        tx.prepare_cached("INSERT OR REPLACE INTO labels(request_id,key,label,timestamp,source,annotation) VALUES(?,?,?,?,?,?)")?.execute(params![id,key,value,label["timestamp"].as_i64().unwrap_or_else(|| Utc::now().timestamp_millis()),label["source"].as_str().unwrap_or("import"),serde_json::to_string(label)?])?;
                    }
                }
            }
        }
        self.prune_in_transaction(&tx, false)?;
        tx.commit()?;
        Ok(inserted)
    }

    pub fn add_label(&self, id: &str, key: &str, label: &str) -> Result<LabelOutcome> {
        if !["correct", "incorrect", "unknown"].contains(&label) {
            bail!("Label must be correct, incorrect or unknown");
        }
        Self::add_label_from(&mut self.writer_connection()?, id, key, label)
    }

    pub(super) fn add_label_from(
        connection: &mut Connection,
        id: &str,
        key: &str,
        label: &str,
    ) -> Result<LabelOutcome> {
        // Hold the writer reservation through classification so retention or
        // deletion cannot remove the answer between its lookup and the write.
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "INSERT OR REPLACE INTO labels(request_id,key,label,timestamp,source) SELECT request_id,key,?,?,? FROM answers WHERE request_id=? AND key=?",
            params![label, Utc::now().timestamp_millis(), "local-reviewer", id, key],
        )?;
        let outcome = if changed == 1 {
            LabelOutcome::Saved
        } else {
            let request_exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?)",
                [id],
                |row| row.get(0),
            )?;
            if request_exists {
                LabelOutcome::AnswerNotFound
            } else {
                LabelOutcome::RequestNotFound
            }
        };
        transaction.commit()?;
        Ok(outcome)
    }
}
