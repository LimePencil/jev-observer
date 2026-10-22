//! Build observation records from bounded, privacy-filtered captures.

use super::capture_transport::{CaptureDecodeError, DECODE_ERROR_MESSAGE, decoded_values};
use super::identity::{fingerprint, group_fingerprint, jgrep_family};
use super::privacy::{
    REDACTED, REDACTION_COLLISION_MESSAGE, REDACTION_LIMIT_MESSAGE, REDACTION_LIMITS,
    REDACTION_VALIDATION_MESSAGE, RedactionCollisionError, RedactionValidationError,
    privacy_capture_envelope,
};
use super::types::{Capture, NormalizeOptions, Protocol};
use super::validation::{finite, kind, probability, token_count, validate_answer};
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
            if error.is::<super::decisions::ProtocolShapeError>() {
                "Decisions requires ordered question and answer arrays"
            } else if error.is::<RedactionCollisionError>() {
                REDACTION_COLLISION_MESSAGE
            } else if error.is::<RedactionValidationError>() {
                REDACTION_VALIDATION_MESSAGE
            } else if error.is::<CaptureDecodeError>() {
                DECODE_ERROR_MESSAGE
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
    let (mut raw_request, mut response) = decoded_values(capture, options.capture_state)?;
    if options.protocol == Protocol::OpenaiDecisions {
        anyhow::ensure!(
            super::decisions::valid_wire_shape(
                &raw_request,
                &response,
                (200..300).contains(&capture.status)
            ),
            super::decisions::ProtocolShapeError
        );
        super::decisions::ordinal_maps(&mut raw_request, &mut response);
    }
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
        if let Some(profile) = &options.model_profile {
            envelope["model_profile"] = json!(profile);
        }
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
        if let Some(profile) = &options.model_profile {
            anyhow::ensure!(
                safe["model_profile"].as_str() == Some(profile.as_str()),
                RedactionValidationError
            );
        }
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
    let response = if options.protocol == Protocol::CloudflareSystemone {
        &safe["response"]["result"]
    } else {
        &safe["response"]
    };
    let source = safe["source"].as_str().unwrap_or("default");
    let task = &safe["task_version"];
    let context_redacted = context_redacted(capture, safe);
    let questions = request["questions"].as_object();
    let raw_answers = response["answers"].as_object();
    let profile = options.validation_profile();
    let semantic_assets = super::identity::semantic_asset_questions(raw_request);
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
        let kind = if options.protocol == Protocol::OpenaiDecisions && definition["type"] == "predicate" { "noul" } else { kind(definition, raw) };
        let definition_id = fingerprint("def_v1", definition);
        let presentation_id = fingerprint("presentation_v1", &json!(serde_json::to_string(definition).expect("JSON value")));
        let definition_redacted = definition_redacted(raw_request, safe, key, context_redacted) || semantic_assets.contains(key);
        // Hidden semantic differences must not collapse into one series.
        // Request isolation remains recomputable after redacted export.
        let isolation = super::identity::group_context(options, &safe["id"], definition_redacted);
        let group_id = group_fingerprint(&json!(source), &json!(key), &json!(definition_id), &json!(presentation_id), task, &isolation);
        let candidates = if options.protocol == Protocol::OpenaiDecisions { if kind == "choice" { &definition["choices"] } else { &definition["levels"] } } else { &definition["criteria"] };
        let candidate_id = if matches!(kind, "choice" | "score") && !candidates.is_null() {
            json!(fingerprint("candidates_v1", candidates))
        } else { Value::Null };
        let mut warnings = Vec::new();
        let mut errors = if options.protocol == Protocol::OpenaiDecisions { super::decisions::validate(definition, raw, &mut warnings) } else { validate_answer(definition, raw, kind, &profile, &mut warnings) };
        if options.protocol == Protocol::OpenaiDecisions && questions.map_or(0, |items| items.len()) != raw_answers.map_or(0, |items| items.len()) {
            errors.push("Decision answer count differs from its ordered questions".into());
        }
        if options.protocol == Protocol::OpenaiDecisions && questions.is_some_and(|items| !(1..=400).contains(&items.len())) {
            errors.push("Decisions requires one to 400 ordered questions".into());
        }
        if options.protocol == Protocol::CloudflareSystemone && safe["response"]["success"] != true {
            errors.push("Cloudflare did not report a successful result envelope".into());
        }
        if !(200..300).contains(&capture.status) { errors.push("Request did not complete with a successful HTTP status".into()); }
        if capture.transport_error.is_some() { errors.push("Request had a transport error".into()); }
        if !capture.capture_complete { errors.push("Capture is incomplete".into()); }
        let value = match kind {
            "choice" => raw.get("choice").filter(|v| v.is_string() || (options.protocol == Protocol::OpenaiDecisions && v.is_boolean())).cloned(),
            "score" => raw.get("score").filter(|v| v.is_number()).cloned(),
            "noul" => raw.get(if options.protocol == Protocol::OpenaiDecisions { "probability" } else { "noul" }).filter(|v| v.is_number()).cloned(),
            "box" => raw.get("box").filter(|v| v.is_object()).map(|value| json!({"top":value["top"],"bottom":value["bottom"],"left":value["left"],"right":value["right"]})),
            _ => None,
        }.unwrap_or(Value::Null);
        let family = if definition_redacted { Value::Null } else { jgrep_family(source, key, definition, task, capture.adapter.as_deref()).unwrap_or(Value::Null) };
        json!({
            "key": key, "kind": kind, "group_id": group_id,
            "definition_id": definition_id, "presentation_id": presentation_id,
            "candidate_id": candidate_id, "task_version": task,
            "definition": definition, "definition_redacted": definition_redacted, "value": value,
            "choice_key": if options.protocol == Protocol::OpenaiDecisions && kind == "choice" { super::decisions::choice_key(&value) } else { None },
            "probabilities": if kind == "box" { Value::Null } else if options.protocol == Protocol::OpenaiDecisions && matches!(kind,"choice"|"score") { super::decisions::probabilities(raw,kind) } else { raw.get("probabilities").filter(|v| v.is_object()).cloned().unwrap_or(Value::Null) },
            "confidence": if matches!(kind, "choice" | "score" | "box") { probability(&raw["confidence"]) } else { None },
            "box_found": if kind == "box" && profile.box_presence { raw["found"].as_bool() } else { None },
            "box_present": if kind == "box" && profile.box_presence { probability(&raw["present"]) } else { None },
            "valid": errors.is_empty(), "warnings": warnings, "error": if errors.is_empty() { None } else { Some(errors.join("; ")) },
            "family_id": family["id"], "family_name": family["name"],
            "adapter": if options.protocol == Protocol::OpenaiDecisions { json!("openai-decisions-v1") } else if family.is_null() { Value::Null } else { json!("jgrep-v1") },
            "instance_ref": family["instance_ref"],
            "mapping_reason": if semantic_assets.contains(key) { json!("Question references external media; its semantic content is not in the retained definition, so this observation is isolated without hashing private input") } else if definition_redacted { json!("Definition or grouping context was redacted; equivalence cannot be established, so this observation is isolated and family pooling is disabled") } else { family["reason"].clone() },
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
    let input_field = if options.protocol == Protocol::OpenaiDecisions {
        "input"
    } else {
        "state"
    };
    let normalization_error = if !request.is_object() {
        Some("Request body is not a complete JSON object")
    } else if !request["questions"].is_object() {
        Some("Request questions is not an object")
    } else if options.protocol == Protocol::OpenaiDecisions
        && questions.is_some_and(|items| items.is_empty() || items.len() > 400)
    {
        Some("Decisions requires one to 400 ordered questions")
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
        "model_profile": options.model_profile, "protocol": options.protocol,
        "model": response.get("model").filter(|v| v.is_string()),
        "requested_model": request.get("model").filter(|v| v.is_string()),
        "status": if capture.status == 0 { None } else { Some(capture.status) },
        "duration_ms": if capture.duration_ms.is_finite() && capture.duration_ms >= 0.0 { Some(capture.duration_ms) } else { None },
        "input_tokens": input_tokens, "output_tokens": output_tokens,
        "cost_usd": cost, "cost_basis": cost_basis,
        "price": if cost_basis == Some("configured_estimate") { json!({"input_per_million": options.input_price_per_million, "output_per_million": options.output_price_per_million}) } else { Value::Null },
        "source_event_id": Value::Null, "import_format": Value::Null,
        "capture_complete": capture.capture_complete,
        "state_retained": options.capture_state && request.get(input_field).is_some_and(|v| !v.is_null()),
        "state": if options.capture_state { request.get(input_field).cloned().unwrap_or(Value::Null) } else { Value::Null },
        "transport_error": safe["transport_error"], "sample": false,
        "answers": [], "actions": [], "labels": [],
        "normalization_error": normalization_error,
        "request_extra": extras(request, &["model", input_field, "questions"]),
        "response_extra": extras(response, &["model", "answers", "usage"]),
        "protocol_metadata": if options.protocol == Protocol::CloudflareSystemone { extras(&safe["response"], &["result"]) } else { Value::Null },
        "usage_extra": extras(&response["usage"], &["input_tokens", "output_tokens"])
    });
    record["answers"] = Value::Array(answers);
    record
}
