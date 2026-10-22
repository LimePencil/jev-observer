//! Build observation records from bounded, privacy-filtered captures.

use super::identity::{fingerprint, group_fingerprint, jgrep_family};
use super::privacy::{
    REDACTED, REDACTION_COLLISION_MESSAGE, REDACTION_LIMIT_MESSAGE, REDACTION_LIMITS,
    REDACTION_VALIDATION_MESSAGE, RedactionCollisionError, RedactionValidationError,
    privacy_capture_envelope,
};
use super::types::{Capture, NormalizeOptions};
use super::validation::{finite, kind, probability, token_count, validate_answer};
use crate::catalog;
use anyhow::Result;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeSet;

fn extras(value: &Value, excluded: &[&str]) -> Value {
    value.as_object().map_or(Value::Null, |map| {
        Value::Object(
            map.iter()
                .filter(|(key, _)| !excluded.contains(&key.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    })
}

/// Normalize a bounded copy. This never mutates or reconstructs the forwarded bytes.
pub fn normalize(capture: &Capture, options: &NormalizeOptions) -> Value {
    match normalize_with_secrets(capture, options, &[]) {
        Ok(record) => record,
        Err(error) => privacy_failed_capture(
            capture,
            if error.is::<RedactionCollisionError>() {
                REDACTION_COLLISION_MESSAGE
            } else if error.is::<RedactionValidationError>() {
                REDACTION_VALIDATION_MESSAGE
            } else {
                REDACTION_LIMIT_MESSAGE
            },
        ),
    }
}

fn privacy_failed_capture(capture: &Capture, reason: &'static str) -> Value {
    // Retain transport facts without cloning the captured bodies or copying
    // any original text into the fallback. A fresh identity cannot collapse
    // unrelated failed captures, even when their original IDs contained a
    // credential. Use the ordinary builder to keep its record schema intact.
    let safe_capture = Capture {
        id: format!("redacted_v1_{}", uuid::Uuid::new_v4().simple()).into(),
        timestamp: capture.timestamp,
        source: REDACTED.into(),
        status: capture.status,
        duration_ms: capture.duration_ms,
        capture_complete: false,
        transport_error: capture.transport_error.as_ref().map(|_| REDACTED.into()),
        ..Default::default()
    };
    let request = json!({"questions": {}});
    let safe = json!({
        "request": request, "response": {"answers": {}}, "id": safe_capture.id,
        "source": safe_capture.source, "task_version": null,
        "transport_error": safe_capture.transport_error
    });
    let options = NormalizeOptions {
        provider: Some("unknown".into()),
        ..Default::default()
    };
    let mut record = normalize_prepared(&safe_capture, &options, &request, &safe);
    record["normalization_error"] = json!(reason);
    record
}

fn normalize_with_secrets(
    capture: &Capture,
    options: &NormalizeOptions,
    import_secrets: &[String],
) -> Result<Value> {
    let mut remaining = REDACTION_LIMITS.work;
    let (raw_request, safe) = prepare_capture(capture, options, import_secrets, &mut remaining)?;
    Ok(normalize_prepared(capture, options, &raw_request, &safe))
}

fn prepare_capture(
    capture: &Capture,
    options: &NormalizeOptions,
    import_secrets: &[String],
    remaining: &mut usize,
) -> Result<(Value, Value)> {
    let raw_request: Value = serde_json::from_slice(&capture.request).unwrap_or(Value::Null);
    let response: Value = serde_json::from_slice(&capture.response).unwrap_or(Value::Null);
    prepare_values(
        capture,
        options,
        import_secrets,
        raw_request,
        response,
        remaining,
    )
}

pub(super) fn prepare_values(
    capture: &Capture,
    options: &NormalizeOptions,
    import_secrets: &[String],
    raw_request: Value,
    response: Value,
    remaining: &mut usize,
) -> Result<(Value, Value)> {
    let explicit_secrets: Vec<&str> = [
        capture.secret.as_deref(),
        capture.local_token.as_deref(),
        capture.access_token.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(import_secrets.iter().map(String::as_str))
    .collect();
    let safe = {
        let mut envelope = json!({
            "request": raw_request, "response": null, "id": capture.id,
            "source": capture.source, "task_version": capture.task_version,
            "transport_error": capture.transport_error
        });
        envelope["response"] = response;
        if capture.id.generated {
            // A local UUID or computed fingerprint is structure, not captured
            // secret text. Exclude it from secret discovery too, so a custom
            // private key named `id` still applies only to supplied data.
            envelope
                .as_object_mut()
                .expect("capture envelope")
                .remove("id");
        }
        let mut safe =
            privacy_capture_envelope(&mut envelope, options, &explicit_secrets, remaining)?;
        if capture.id.generated {
            // Restore before grouping: redacted definitions use this identity
            // to keep unrelated observations in separate groups.
            safe["id"] = json!(capture.id);
        }
        safe
    };
    Ok((raw_request, safe))
}

pub(super) fn context_redacted(capture: &Capture, safe: &Value) -> bool {
    safe["source"]
        .as_str()
        .is_some_and(|source| source.contains(REDACTED))
        || safe["task_version"].to_string().contains(REDACTED)
        || safe["source"] != capture.source
        || safe["task_version"] != json!(capture.task_version)
}

pub(super) fn definition_redacted(
    raw_request: &Value,
    safe: &Value,
    key: &str,
    context_redacted: bool,
) -> bool {
    let definition = &safe["request"]["questions"][key];
    context_redacted
        || key.contains(REDACTED)
        || raw_request["questions"]
            .get(key)
            .is_some_and(|original| original != definition)
        || definition.to_string().contains(REDACTED)
}

pub(super) fn normalize_prepared(
    capture: &Capture,
    options: &NormalizeOptions,
    raw_request: &Value,
    safe: &Value,
) -> Value {
    let request = &safe["request"];
    let response = &safe["response"];
    let source = safe["source"].as_str().unwrap_or("default");
    let task = &safe["task_version"];
    let context_redacted = context_redacted(capture, safe);
    let questions = request["questions"].as_object();
    let raw_answers = response["answers"].as_object();
    let profile = catalog::profile(options.provider.as_deref().unwrap_or("typesafe"));
    let mut keys: Vec<String> = questions
        .map(|q| q.keys().cloned().collect())
        .unwrap_or_default();
    let mut seen: BTreeSet<String> = keys.iter().cloned().collect();
    if let Some(answers) = raw_answers {
        for key in answers.keys() {
            if seen.insert(key.clone()) {
                keys.push(key.clone());
            }
        }
    }
    let answers: Vec<Value> = keys.iter().map(|key| {
        let definition = questions.and_then(|q| q.get(key)).unwrap_or(&Value::Null);
        let raw = raw_answers.and_then(|a| a.get(key)).unwrap_or(&Value::Null);
        let kind = kind(definition, raw);
        let definition_id = fingerprint("def_v1", definition);
        let presentation_id = fingerprint("presentation_v1", &json!(serde_json::to_string(definition).expect("JSON value")));
        let definition_redacted = definition_redacted(raw_request, safe, key, context_redacted);
        // Hidden semantic differences must not collapse into one series.
        // Request isolation remains recomputable after redacted export.
        let isolation = if definition_redacted { safe["id"].clone() } else { Value::Null };
        let group_id = group_fingerprint(&json!(source), &json!(key), &json!(definition_id), &json!(presentation_id), task, &isolation);
        let candidate_id = if matches!(kind, "choice" | "score") && !definition["criteria"].is_null() {
            json!(fingerprint("candidates_v1", &definition["criteria"]))
        } else { Value::Null };
        let mut warnings = Vec::new();
        let mut errors = validate_answer(definition, raw, kind, &profile, &mut warnings);
        if !(200..300).contains(&capture.status) { errors.push("Request did not complete with a successful HTTP status".into()); }
        if capture.transport_error.is_some() { errors.push("Request had a transport error".into()); }
        if !capture.capture_complete { errors.push("Capture is incomplete".into()); }
        let value = match kind {
            "choice" => raw.get("choice").filter(|v| v.is_string()).cloned(),
            "score" => raw.get("score").filter(|v| v.is_number()).cloned(),
            "noul" => raw.get("noul").filter(|v| v.is_number()).cloned(),
            _ => None,
        }.unwrap_or(Value::Null);
        let family = if definition_redacted { Value::Null } else { jgrep_family(source, key, definition, task, capture.adapter.as_deref()).unwrap_or(Value::Null) };
        json!({
            "key": key, "kind": kind, "group_id": group_id,
            "definition_id": definition_id, "presentation_id": presentation_id,
            "candidate_id": candidate_id, "task_version": task,
            "definition": definition, "definition_redacted": definition_redacted, "value": value,
            "probabilities": raw.get("probabilities").filter(|v| v.is_object()),
            "confidence": if matches!(kind, "choice" | "score") { probability(&raw["confidence"]) } else { None },
            "valid": errors.is_empty(), "warnings": warnings, "error": if errors.is_empty() { None } else { Some(errors.join("; ")) },
            "family_id": family["id"], "family_name": family["name"],
            "adapter": if family.is_null() { Value::Null } else { json!("jgrep-v1") },
            "instance_ref": family["instance_ref"],
            "mapping_reason": if definition_redacted { json!("Definition or grouping context was redacted; equivalence cannot be established, so this observation is isolated and family pooling is disabled") } else { family["reason"].clone() },
            "raw_answer": raw
        })
    }).collect();
    let input_tokens = token_count(&response["usage"]["input_tokens"]);
    let output_tokens = token_count(&response["usage"]["output_tokens"]);
    let estimated_cost = match (
        input_tokens,
        output_tokens,
        options.input_price_per_million,
        options.output_price_per_million,
    ) {
        (Some(input), Some(output), Some(ir), Some(or))
            if ir.is_finite() && or.is_finite() && ir >= 0.0 && or >= 0.0 =>
        {
            let value = (input as f64 * ir + output as f64 * or) / 1_000_000.0;
            value.is_finite().then_some(value)
        }
        _ => None,
    };
    // Only this adapter has a verified USD meaning for `usage.cost`.
    // Custom upstreams can use the same field name for credits or other units.
    let reported_cost = (options.provider.as_deref() == Some("openrouter"))
        .then(|| finite(&response["usage"]["cost"]).filter(|value| *value >= 0.0))
        .flatten();
    let cost = reported_cost.or(estimated_cost);
    let cost_basis = if reported_cost.is_some() {
        Some("provider_reported")
    } else {
        estimated_cost.map(|_| "configured_estimate")
    };
    let normalization_error = if !request.is_object() {
        Some("Request body is not a complete JSON object")
    } else if !request["questions"].is_object() {
        Some("Request questions is not an object")
    } else if !response.is_object() {
        Some("Response body is not a complete JSON object")
    } else if (200..300).contains(&capture.status) && !response["answers"].is_object() {
        Some("Successful response has no answers object")
    } else {
        None
    };
    let mut record = json!({
        "schema_version": 1, "event_kind": "request", "id": safe["id"], "timestamp": capture.timestamp,
        "source": source, "provider": options.provider.as_deref().unwrap_or("typesafe"),
        "model": response.get("model").filter(|v| v.is_string()),
        "requested_model": request.get("model").filter(|v| v.is_string()),
        "status": if capture.status == 0 { None } else { Some(capture.status) },
        "duration_ms": if capture.duration_ms.is_finite() && capture.duration_ms >= 0.0 { Some(capture.duration_ms) } else { None },
        "input_tokens": input_tokens, "output_tokens": output_tokens,
        "cost_usd": cost, "cost_basis": cost_basis,
        "price": if cost_basis == Some("configured_estimate") { json!({"input_per_million": options.input_price_per_million, "output_per_million": options.output_price_per_million}) } else { Value::Null },
        "source_event_id": Value::Null, "import_format": Value::Null,
        "capture_complete": capture.capture_complete,
        "state_retained": options.capture_state && request.get("state").is_some_and(|v| !v.is_null()),
        "state": if options.capture_state { request.get("state").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "transport_error": safe["transport_error"], "sample": false,
        "answers": [], "actions": [], "labels": [],
        "normalization_error": normalization_error,
        "request_extra": extras(request, &["model", "state", "questions"]),
        "response_extra": extras(response, &["model", "answers", "usage"]),
        "usage_extra": extras(&response["usage"], &["input_tokens", "output_tokens"])
    });
    record["answers"] = Value::Array(answers);
    record
}
