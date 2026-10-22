//! Validate, budget, and normalize persisted captures and receipts.

mod observer;
mod receipt;
use observer::import_observer;
use receipt::import_receipt;

use super::budget::{
    IMPORT_LIMITS, ImportBudget, ImportLimitError, ImportLimits, ImportRecordBudget, limited_add,
};
use super::normalize::{context_redacted, definition_redacted, normalize_prepared, prepare_values};
use super::privacy::{
    REDACTION_LIMITS, collect_secrets, privacy_observer, privacy_with_input_filter,
    remove_receipt_inputs,
};
use super::types::answer_keys;
use super::types::{Capture, CaptureId, NormalizeOptions};
use super::validation::finite;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use anyhow::ensure;
use serde_json::Value;
use serde_json::json;

pub(super) const MAX_IMPORT_RECORDS: usize = 10_000;
/// Match the dashboard's JavaScript Date domain and HTTP date filters.
pub fn valid_timestamp(timestamp: i64) -> bool {
    (-8_640_000_000_000_000..=8_640_000_000_000_000).contains(&timestamp)
}

pub const IMPORT_IDENTITY_ERROR_MESSAGE: &str = "Import rejected because privacy filtering changes an event identity. Use stable, non-sensitive IDs and source names.";

#[derive(Debug)]
pub struct ImportIdentityError;

impl std::fmt::Display for ImportIdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(IMPORT_IDENTITY_ERROR_MESSAGE)
    }
}

impl std::error::Error for ImportIdentityError {}

fn normalize_import(
    capture: &Capture,
    options: &NormalizeOptions,
    import_secrets: &[String],
    budget: &mut ImportRecordBudget<'_>,
    charge_input: bool,
) -> Result<Value> {
    // Imported Observer envelopes may contain the additional wrappers our
    // own output adds. Never silently replace their reconstructed bodies with
    // null because a raw-provider parser has a smaller recursion allowance.
    let raw_request = crate::normalized_json::from_slice(&capture.request)?;
    let response = crate::normalized_json::from_slice(&capture.response)?;
    let (raw_request, safe) = prepare_values(
        capture,
        options,
        import_secrets,
        raw_request,
        response,
        &mut budget.batch.redaction_remaining,
    )?;
    if charge_input {
        budget.value(&safe)?;
    }
    // Use the sanitized lengths: short secrets can expand to redaction markers.
    // Charge before fingerprints and the answer Vec replicate this context.
    let source = safe["source"].as_str().unwrap_or("default");
    let context = limited_add(
        budget.size(source)?,
        budget.size(&safe["task_version"])?,
        budget.batch.limits.record,
        "normalization work",
    )?;
    let context_redacted = context_redacted(capture, &safe);
    let isolation = budget.size(&safe["id"])?;
    for key in answer_keys(&safe["request"], &safe["response"]) {
        budget.charge(context)?;
        if definition_redacted(&raw_request, &safe, key, context_redacted) {
            budget.charge(isolation)?;
        }
    }
    Ok(normalize_prepared(capture, options, &raw_request, &safe))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("{key} must be a nonempty string"))
}

fn observer_event_id<'a>(record: &'a Value, id: &'a str) -> &'a str {
    record["source_event_id"]
        .as_str()
        .filter(|event| !event.is_empty())
        .unwrap_or(id)
}

// External identities must survive privacy unchanged: replacing them with a
// shared marker would turn distinct events into successful duplicate imports.
fn check_import_identity(original: &Value, retained: &Value, format: &str) -> Result<()> {
    if format == "observer-jsonl" {
        let id = required_string(original, "id")?;
        let source = required_string(original, "source")?;
        ensure!(
            retained["id"].as_str() == Some(id)
                && retained["source"].as_str() == Some(source)
                && observer_event_id(retained, id) == observer_event_id(original, id),
            ImportIdentityError
        );
    } else {
        let decision = required_string(original, "decision_id")?;
        ensure!(
            retained["decision_id"].as_str() == Some(decision),
            ImportIdentityError
        );
    }
    Ok(())
}

fn check_persisted_import_identity(
    record: &Value,
    id: &str,
    source: &str,
    event: &str,
    format: &str,
) -> Result<()> {
    ensure!(
        record["id"].as_str() == Some(id)
            && record["source"].as_str() == Some(source)
            && record["source_event_id"].as_str() == Some(event)
            && record["import_format"].as_str() == Some(format),
        ImportIdentityError
    );
    Ok(())
}

fn nullable_number(value: &Value, key: &str) -> Result<()> {
    ensure!(
        value[key].is_null() || finite(&value[key]).is_some_and(|n| n >= 0.0),
        "{key} must be a nonnegative number or null"
    );
    Ok(())
}

fn charge_annotations(
    record: &Value,
    key: &str,
    source: &str,
    event_id: &str,
    budget: &mut ImportRecordBudget<'_>,
) -> Result<()> {
    let annotations = record[key]
        .as_array()
        .with_context(|| format!("{key} must be an array"))?;
    let source_bytes = budget.size(source)?;
    let event_bytes = budget.size(event_id)?;
    budget.value(&record[key])?;
    for annotation in annotations {
        if annotation.get("source").is_none() {
            budget.charge(source_bytes)?;
        }
        if annotation.get("source_event_id").is_none() {
            budget.charge(event_bytes)?;
        }
    }
    Ok(())
}

fn imported_annotations(record: &Value, key: &str, source: &str, event_id: &str) -> Result<Value> {
    let annotations = record[key]
        .as_array()
        .with_context(|| format!("{key} must be an array"))?;
    let mut out = Vec::new();
    for annotation in annotations {
        ensure!(annotation.is_object(), "{key} entries must be objects");
        if key == "labels" {
            annotation["key"].as_str().context("key must be a string")?;
            ensure!(
                matches!(
                    annotation["label"].as_str(),
                    Some("correct" | "incorrect" | "unknown")
                ),
                "Unsupported label value"
            );
        }
        let mut annotation = annotation.clone();
        let obj = annotation.as_object_mut().expect("checked object");
        obj.entry("source").or_insert_with(|| json!(source));
        obj.entry("source_event_id")
            .or_insert_with(|| json!(event_id));
        out.push(annotation);
    }
    Ok(json!(out))
}

/// A raw, explicitly identified call, including mapped library decisions. The
/// envelope records observations; supplied validity, cost and group IDs are ignored.
pub(super) fn import_capture(
    record: &Value,
    options: &NormalizeOptions,
    budget: &mut ImportRecordBudget<'_>,
) -> Result<Value> {
    let event_id = required_string(record, "id")?;
    let source = required_string(record, "source")?;
    let provider = required_string(record, "provider")?;
    ensure!(
        provider.len() <= 64
            && provider
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')),
        "Invalid provider name"
    );
    let timestamp = record["timestamp"]
        .as_i64()
        .filter(|timestamp| valid_timestamp(*timestamp))
        .context("timestamp must be valid epoch milliseconds")?;
    let status = record["status"]
        .as_u64()
        .filter(|status| (100..=599).contains(status))
        .context("status must be an HTTP status from 100 to 599")?;
    let complete = record["capture_complete"]
        .as_bool()
        .context("capture_complete must be a boolean")?;
    ensure!(
        record["request"].is_object() && record["request"]["questions"].is_object(),
        "request must contain named System One questions"
    );
    ensure!(record["response"].is_object(), "response must be an object");
    for key in ["transport_error", "task_version"] {
        ensure!(
            record[key].is_null() || record[key].is_string(),
            "{key} must be a string or null"
        );
    }
    nullable_number(record, "duration_ms")?;
    let id = CaptureId::raw_import(source, provider, event_id);
    let capture = Capture {
        id,
        timestamp,
        source: source.to_owned(),
        task_version: record["task_version"].as_str().map(str::to_owned),
        status: status as u16,
        duration_ms: finite(&record["duration_ms"]).unwrap_or(f64::NAN),
        request: serde_json::to_vec(&record["request"])?,
        response: serde_json::to_vec(&record["response"])?,
        capture_complete: complete,
        transport_error: record["transport_error"].as_str().map(str::to_owned),
        ..Default::default()
    };
    let mut imported_options = options.clone();
    imported_options.provider = Some(provider.to_ascii_lowercase());
    let mut import_secrets = Vec::new();
    collect_secrets(record, options, &mut import_secrets);
    let mut out = normalize_import(&capture, &imported_options, &import_secrets, budget, true)?;
    // The generated fingerprint retains no caller text. Preserve it through
    // privacy filtering so distinct calls cannot become false duplicates.
    out["source_event_id"] = out["id"].clone();
    out["import_format"] = json!("systemone-capture");
    ensure!(
        record["sample"].is_null() || record["sample"].is_boolean(),
        "sample must be a boolean or null"
    );
    out["sample"] = json!(record["sample"] == true);
    Ok(out)
}

/// Import is all-or-nothing validation. Storage owns deduplication by explicit
/// source event identity; equal payloads are never presumed to be the same call.
pub fn import_records(text: &str, format: &str, options: &NormalizeOptions) -> Result<Vec<Value>> {
    import_records_with_limits(text, format, options, IMPORT_LIMITS)
}

pub(super) fn import_records_with_limits(
    text: &str,
    format: &str,
    options: &NormalizeOptions,
    limits: ImportLimits,
) -> Result<Vec<Value>> {
    ensure!(
        matches!(
            format,
            "observer-jsonl" | "jevrouter-receipt" | "systemone-capture"
        ),
        "Unsupported import format: {format}"
    );
    let mut values = Vec::new();
    if format == "jevrouter-receipt" {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            match value {
                Value::Array(items) => values = items,
                Value::Object(_) => values.push(value),
                _ => bail!("Receipt must be a JSON object or array"),
            }
        } else {
            for (index, line) in text
                .lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
            {
                ensure!(
                    values.len() < MAX_IMPORT_RECORDS,
                    ImportLimitError("record count")
                );
                values.push(
                    serde_json::from_str(line)
                        .with_context(|| format!("Invalid JSON at line {}", index + 1))?,
                );
            }
        }
    } else {
        for (index, line) in text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
        {
            ensure!(
                values.len() < MAX_IMPORT_RECORDS,
                ImportLimitError("record count")
            );
            let record = if format == "observer-jsonl" {
                crate::normalized_json::from_slice(line.as_bytes())
            } else {
                serde_json::from_str(line)
            };
            values.push(record.with_context(|| format!("Invalid JSON at line {}", index + 1))?);
        }
    }
    ensure!(!values.is_empty(), "Import contains no records");
    ensure!(
        values.len() <= MAX_IMPORT_RECORDS,
        ImportLimitError("record count")
    );
    let mut batch = ImportBudget {
        limits,
        work: 0,
        normalized: 0,
        redaction_remaining: REDACTION_LIMITS.work,
    };
    values
        .iter_mut()
        .enumerate()
        .map(|(index, record)| {
            let mut budget = ImportRecordBudget {
                batch: &mut batch,
                work: 0,
            };
            budget.structure(record, format)?;
            if format == "systemone-capture" {
                // normalize must see the original definition to detect redaction
                // and isolate hidden semantic changes before fingerprints are made.
                let out = import_capture(record, options, &mut budget)
                    .with_context(|| format!("Invalid record {}", index + 1))?;
                budget.finish(&out)?;
                return Ok(out);
            }
            // Scrub before normalization so all recomputed identities describe the
            // retained definition, and credential echoes cannot leak into metadata.
            let mut safe = if format == "observer-jsonl" {
                privacy_observer(record, options, &mut budget.batch.redaction_remaining)?
            } else {
                privacy_with_input_filter(
                    record,
                    options,
                    &[],
                    &mut budget.batch.redaction_remaining,
                    remove_receipt_inputs,
                )?
            };
            // Before reconstruction/cloning, bound even privacy-expanded input.
            // Observer derived answer fields are not retained wholesale: its
            // reconstructed input and provenance are charged by import_observer.
            budget.size(&safe)?;
            if format == "jevrouter-receipt" {
                budget.value(&safe)?;
            }
            check_import_identity(record, &safe, format)?;
            if format == "observer-jsonl"
                && let (Some(original), Some(retained)) = (
                    record.get("answers").and_then(Value::as_array),
                    safe.get_mut("answers").and_then(Value::as_array_mut),
                )
            {
                // Removed headers leave no marker behind. Preserve evidence of
                // any changed definition before the unsanitized record is lost;
                // imported validity/grouping claims cannot make it safe to pool.
                for (original, retained) in original.iter().zip(retained) {
                    if original.get("definition") != retained.get("definition")
                        && let Some(answer) = retained.as_object_mut()
                    {
                        answer.insert("definition_redacted".into(), json!(true));
                    }
                }
            }
            let result = if format == "observer-jsonl" {
                import_observer(&safe, options, &mut budget)
            } else {
                import_receipt(&safe, options, &mut budget)
            };
            let out = result.with_context(|| format!("Invalid record {}", index + 1))?;
            budget.finish(&out)?;
            Ok(out)
        })
        .collect()
}
