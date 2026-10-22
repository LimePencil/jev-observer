//! Preserve JevRouter receipts as actions, without inventing inference observations.

use super::super::{
    identity::fingerprint,
    privacy::{privacy, remove_receipt_inputs},
    types::Capture,
};
use super::{
    ImportRecordBudget, NormalizeOptions, check_persisted_import_identity, normalize_import,
    required_string,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};

pub(super) fn import_receipt(
    record: &Value,
    options: &NormalizeOptions,
    budget: &mut ImportRecordBudget<'_>,
) -> Result<Value> {
    let decision_id = required_string(record, "decision_id")?;
    required_string(record, "request_id")?;
    ensure!(
        record["mode"] == "decision_only",
        "Only JevRouter decision receipts are supported (plans are separate records)"
    );
    ensure!(
        matches!(
            record["status"].as_str(),
            Some("selected" | "needs_confirmation" | "no_decision")
        ),
        "Invalid JevRouter decision status"
    );
    ensure!(
        record["decision"]["kind"] == "choice" && record["decision"]["candidates"].is_array(),
        "Invalid JevRouter decision"
    );
    required_string(&record["provenance"], "jev_provider")?;
    ensure!(
        record["execution"].is_object() && record["fallback"].is_object(),
        "Missing receipt execution/fallback provenance"
    );
    ensure!(
        record["raw_jev"].is_null() || record["raw_jev"].is_object(),
        "raw_jev must be an object or null"
    );
    let source = "jevrouter";
    let provider = &record["provenance"]["jev_provider"];
    // The original receipt and its action retain overlapping provenance. Charge
    // the second copy before cloning, then charge envelope extensions retained
    // a third time. Opted-out inputs were already removed before privacy scrub.
    budget.value(record)?;
    if let Some(raw) = record["raw_jev"].as_object() {
        for (key, value) in raw {
            if !matches!(key.as_str(), "model" | "answers" | "usage") {
                budget.charge(budget.size(key)?)?;
                budget.value(value)?;
            }
        }
    }
    let response = record
        .get("raw_jev")
        .and_then(Value::as_object)
        .map(|raw| {
            Value::Object(
                raw.iter()
                    .filter(|(key, _)| key.as_str() != "answers")
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        })
        .unwrap_or(json!({}));
    let capture = Capture {
        id: fingerprint("receipt", &json!([source, decision_id])).into(),
        source: source.into(),
        request: serde_json::to_vec(&json!({"questions": {}}))?,
        response: serde_json::to_vec(&response)?,
        duration_ms: f64::NAN,
        // A receipt is not a captured HTTP transaction. Do not fabricate its
        // status, original definitions, model call, or capture completeness.
        ..Capture::default()
    };
    // Receipt answers are provenance, not inference observations. They were
    // already included in the global privacy pass and are preserved below.
    let mut out = normalize_import(&capture, options, &[], budget, false, false)?;
    out["timestamp"] = Value::Null;
    out["timestamp_basis"] = json!("import");
    out["imported_at"] = json!(chrono::Utc::now().timestamp_millis());
    out["provider"] = provider.clone();
    out["source_event_id"] = json!(decision_id);
    out["import_format"] = json!("jevrouter-receipt");
    out["observation_origin"] = json!("imported_receipt_not_verified_live_inference");
    out["event_kind"] = json!("application_action");
    out["answers"] = json!([]);
    out["input_tokens"] = Value::Null;
    out["output_tokens"] = Value::Null;
    out["normalization_error"] = Value::Null;
    out["sample"] = provider
        .as_str()
        .is_some_and(|p| p == "demo" || p == "jevrouter-demo")
        .into();
    out["cost_usd"] = Value::Null;
    out["cost_basis"] = Value::Null;
    out["price"] = Value::Null;
    out["actions"] = json!([{
        "kind": "policy_decision", "key": "tool", "source": "jevrouter-receipt",
        "source_event_id": decision_id, "status": record["status"],
        "decision": record["decision"], "execution": record["execution"],
        "fallback": record["fallback"], "provenance": record["provenance"],
        "raw_jev": record["raw_jev"], "raw_jev_stages": record["raw_jev_stages"]
    }]);
    let mut receipt = record.clone();
    // Inputs are application state too, even though this format uses a
    // different field name. The remaining receipt is retained as provenance.
    if !options.capture_state {
        remove_receipt_inputs(&mut receipt);
        remove_receipt_inputs(&mut out["actions"][0]);
    }
    out["receipt"] = receipt;
    let out = privacy(&out, options, &[], &mut budget.batch.redaction_remaining)?;
    check_persisted_import_identity(&out, &capture.id, source, decision_id, "jevrouter-receipt")?;
    Ok(out)
}
