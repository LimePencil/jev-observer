//! Revalidate an Observer JSONL record without trusting persisted derived fields.

use super::super::{
    identity::group_fingerprint,
    privacy::{privacy_observer, remove_receipt_inputs},
    types::Capture,
    validation::{finite, token_count},
};
use super::{
    ImportRecordBudget, NormalizeOptions, charge_annotations, check_persisted_import_identity,
    imported_annotations, normalize_import, nullable_number, observer_event_id, required_string,
    valid_timestamp,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

fn original_present(answer: &Value, field: &str, presence: &str, legacy: bool) -> Result<bool> {
    let present = answer
        .get(presence)
        .map(|value| {
            value
                .as_bool()
                .with_context(|| format!("{presence} must be a boolean when supplied"))
        })
        .transpose()?
        .unwrap_or(legacy);
    ensure!(
        present || answer[field].is_null(),
        "{presence}=false cannot discard a non-null {field}"
    );
    Ok(present)
}

pub(super) fn import_observer(
    record: &Value,
    options: &NormalizeOptions,
    budget: &mut ImportRecordBudget<'_>,
) -> Result<Value> {
    ensure!(
        record["schema_version"] == 1,
        "Unsupported Observer schema_version"
    );
    for key in [
        "id",
        "timestamp",
        "source",
        "provider",
        "model",
        "requested_model",
        "status",
        "duration_ms",
        "input_tokens",
        "output_tokens",
        "cost_usd",
        "cost_basis",
        "source_event_id",
        "import_format",
        "capture_complete",
        "state_retained",
        "state",
        "transport_error",
        "sample",
        "answers",
        "actions",
        "labels",
    ] {
        ensure!(record.get(key).is_some(), "Missing Observer field: {key}");
    }
    let id = required_string(record, "id")?;
    let source = required_string(record, "source")?;
    for key in ["timestamp", "imported_at"] {
        ensure!(
            record[key].is_null() || record[key].as_i64().is_some_and(valid_timestamp),
            "{key} must be valid epoch milliseconds or null"
        );
    }
    for key in ["duration_ms", "cost_usd"] {
        nullable_number(record, key)?;
    }
    for key in ["input_tokens", "output_tokens"] {
        ensure!(
            record[key].is_null() || token_count(&record[key]).is_some(),
            "{key} must be an integer between zero and {} or null",
            i64::MAX
        );
    }
    ensure!(
        record["status"].is_null()
            || record["status"]
                .as_u64()
                .is_some_and(|n| (100..=599).contains(&n)),
        "status must be a valid HTTP status or null"
    );
    for key in ["capture_complete", "state_retained", "sample"] {
        ensure!(record[key].is_boolean(), "{key} must be boolean");
    }
    for key in [
        "provider",
        "model_profile",
        "protocol",
        "model",
        "requested_model",
        "source_event_id",
        "import_format",
        "transport_error",
        "cost_basis",
    ] {
        ensure!(
            record[key].is_null() || record[key].is_string(),
            "{key} must be a string or null"
        );
    }
    let event_id = observer_event_id(record, id);
    // Charge retained provenance and defaults before reconstructing question
    // maps. Derived export fields are not themselves normalization inputs.
    for key in [
        "provider",
        "cost_basis",
        "model_profile",
        "protocol",
        "price",
        "protocol_metadata",
        "receipt",
        "imported_at",
        "timestamp_basis",
        "observation_origin",
        "normalization_error",
    ] {
        budget.value(&record[key])?;
    }
    budget.charge(budget.size(event_id)?)?;
    for key in ["actions", "labels"] {
        charge_annotations(record, key, "observer-jsonl", event_id, budget)?;
    }
    let answers = record["answers"]
        .as_array()
        .context("answers must be an array")?;
    let mut questions = Map::new();
    let mut raw_answers = Map::new();
    let mut original_answers = BTreeMap::new();
    for answer in answers {
        // Question map keys are arbitrary strings, including the empty string.
        // Preserve the same key domain as normalization and label creation.
        let key = answer["key"].as_str().context("key must be a string")?;
        ensure!(
            original_answers.insert(key, answer).is_none(),
            "Duplicate answer key"
        );
        ensure!(
            answer.get("definition").is_some() && answer.get("raw_answer").is_some(),
            "Answer must retain definition and raw_answer (null allowed)"
        );
        // A supplied null and an absent map entry are different evidence,
        // especially for ordered Decisions question/answer counts. Older
        // exports did not distinguish them, so retain their previous fallback.
        let definition_present = original_present(
            answer,
            "definition",
            "definition_present",
            !answer["definition"].is_null() || answer["raw_answer"].is_null(),
        )?;
        let raw_answer_present = original_present(
            answer,
            "raw_answer",
            "raw_answer_present",
            !answer["raw_answer"].is_null(),
        )?;
        ensure!(
            definition_present || raw_answer_present,
            "Answer must retain an original question or answer entry"
        );
        if definition_present {
            questions.insert(key.to_owned(), answer["definition"].clone());
        }
        if raw_answer_present {
            raw_answers.insert(key.to_owned(), answer["raw_answer"].clone());
        }
    }
    let mut request = record
        .get("request_extra")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    request["model"] = record["requested_model"].clone();
    request["questions"] = Value::Object(questions);
    let input_field = if record["protocol"] == "openai-decisions" {
        "input"
    } else {
        "state"
    };
    request[input_field] = if record["state_retained"] == true {
        record["state"].clone()
    } else {
        Value::Null
    };
    let mut response = record
        .get("response_extra")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    response["model"] = record["model"].clone();
    response["answers"] = Value::Object(raw_answers);
    let mut usage = record
        .get("usage_extra")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    usage["input_tokens"] = record["input_tokens"].clone();
    usage["output_tokens"] = record["output_tokens"].clone();
    response["usage"] = usage;
    if record["protocol"] == "cloudflare-systemone" {
        let mut envelope = record["protocol_metadata"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        envelope.insert("result".into(), response);
        response = Value::Object(envelope);
    }
    let task = answers
        .first()
        .and_then(|a| a["task_version"].as_str())
        .map(str::to_owned);
    ensure!(
        answers
            .iter()
            .all(|a| a["task_version"].as_str() == task.as_deref()),
        "Mixed task versions in one request"
    );
    let adapter = answers
        .iter()
        .any(|a| a["adapter"] == "jgrep-v1")
        .then(|| "jgrep-v1".to_owned());
    let capture = Capture {
        id: id.into(),
        timestamp: record["timestamp"].as_i64().unwrap_or(0),
        source: source.to_owned(),
        task_version: task,
        adapter,
        status: record["status"].as_u64().unwrap_or(0) as u16,
        duration_ms: finite(&record["duration_ms"]).unwrap_or(f64::NAN),
        request: serde_json::to_vec(&request)?,
        response: serde_json::to_vec(&response)?,
        capture_complete: record["capture_complete"] == true,
        transport: None,
        transport_error: record["transport_error"].as_str().map(str::to_owned),
        secret: None,
        local_token: None,
        access_token: None,
    };
    let mut imported_options = options.clone();
    imported_options.provider = record["provider"].as_str().map(str::to_owned);
    imported_options.model_profile = record["model_profile"].as_str().map(str::to_owned);
    ensure!(
        imported_options
            .model_profile
            .as_deref()
            .is_none_or(crate::catalog::known_profile),
        "Unknown model_profile"
    );
    imported_options.protocol = if record["protocol"].is_null() {
        super::super::types::Protocol::default()
    } else {
        serde_json::from_value(record["protocol"].clone())
            .context("Unsupported capture protocol")?
    };
    let mut out = normalize_import(&capture, &imported_options, &[], budget, true, false)?;
    for derived in out["answers"].as_array_mut().expect("normalized answers") {
        // The same index validates uniqueness and retains per-answer privacy
        // evidence without rescanning a potentially large export for each key.
        let original = derived["key"]
            .as_str()
            .and_then(|key| original_answers.get(key));
        // Removed fields leave no marker. A conservative isolation flag can
        // only deny pooling; supplied fingerprints remain untrusted.
        if original.is_some_and(|answer| answer["definition_redacted"] == true)
            && derived["definition_redacted"] != true
        {
            budget.charge(budget.size(source)?)?;
            budget.value(&derived["task_version"])?;
            budget.charge(budget.size(id)?)?;
            derived["definition_redacted"] = json!(true);
            derived["group_id"] = json!(group_fingerprint(
                &json!(source),
                &derived["key"],
                &derived["definition_id"],
                &derived["presentation_id"],
                &derived["task_version"],
                &super::super::identity::group_context(&imported_options, &json!(id), true)
            ));
            for key in ["family_id", "family_name", "adapter", "instance_ref"] {
                if key == "adapter"
                    && imported_options.protocol == super::super::types::Protocol::OpenaiDecisions
                {
                    continue;
                }
                derived[key] = Value::Null;
            }
            derived["mapping_reason"] = json!(
                "Definition or grouping context was redacted; equivalence cannot be established, so this observation is isolated and family pooling is disabled"
            );
        }
    }
    if let Some(event_kind) = record.get("event_kind") {
        ensure!(
            matches!(event_kind.as_str(), Some("request" | "application_action")),
            "Unsupported event_kind"
        );
        out["event_kind"] = event_kind.clone();
        if event_kind == "application_action" {
            ensure!(
                answers.is_empty(),
                "Application action records cannot supply inference answers"
            );
        }
    }
    out["timestamp"] = record["timestamp"].clone();
    out["provider"] = record["provider"].clone();
    out["sample"] = record["sample"].clone();
    out["source_event_id"] = json!(event_id);
    out["import_format"] = json!("observer-jsonl");
    out["cost_usd"] = record["cost_usd"].clone();
    out["cost_basis"] = if record["cost_usd"].is_null() {
        Value::Null
    } else {
        ensure!(
            !record["cost_basis"].as_str().unwrap_or("").is_empty(),
            "Known cost requires its cost_basis"
        );
        record["cost_basis"].clone()
    };
    out["price"] = record.get("price").cloned().unwrap_or(Value::Null);
    out["actions"] = imported_annotations(record, "actions", "observer-jsonl", event_id)?;
    out["labels"] = imported_annotations(record, "labels", "observer-jsonl", event_id)?;
    // Keep additional provenance without treating supplied fingerprints,
    // validity flags or derived values as authoritative.
    for key in [
        "receipt",
        "imported_at",
        "timestamp_basis",
        "observation_origin",
        "normalization_error",
    ] {
        if let Some(value) = record.get(key) {
            out[key] = value.clone();
        }
    }
    if out["timestamp"].is_null() {
        if out["imported_at"].is_null() {
            out["imported_at"] = json!(chrono::Utc::now().timestamp_millis());
        }
        out["timestamp_basis"] = json!("import");
    }
    if !options.capture_state {
        if let Some(receipt) = out.get_mut("receipt") {
            remove_receipt_inputs(receipt);
        }
        for action in out["actions"]
            .as_array_mut()
            .expect("validated annotations")
        {
            remove_receipt_inputs(action);
        }
    }
    let out = privacy_observer(&mut out, options, &mut budget.batch.redaction_remaining)?;
    // Reconstruction and retained provenance introduce later privacy passes.
    // Check the actual stored identity, not only the first filtered input.
    check_persisted_import_identity(&out, id, source, event_id, "observer-jsonl")?;
    Ok(out)
}
