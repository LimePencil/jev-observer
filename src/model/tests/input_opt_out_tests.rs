use super::*;

#[test]
fn discarded_input_secrets_still_obey_the_shared_redaction_limits() {
    let options = NormalizeOptions::default();
    let secrets: Vec<_> = (0..=REDACTION_LIMITS.patterns)
        .map(|index| format!("discarded-credential-{index}"))
        .collect();
    let mut value = json!({"state":{"credentials":secrets}, "extension":"retained"});
    let mut remaining = REDACTION_LIMITS.work;
    let error = privacy_with_input_filter(
        &mut value,
        &options,
        &[],
        &mut remaining,
        clear_input_fields,
    )
    .unwrap_err();
    assert!(error.is::<ImportLimitError>());
    assert!(
        value.get("state").is_some(),
        "limits must be checked before pruning"
    );

    let original = json!({"state":{"password":"ONLY_DISCARDED_QZX"},
        "extension":"ONLY_DISCARDED_QZX"});
    let mut value = original.clone();
    let retained = privacy_with_input_filter(
        &mut value,
        &options,
        &[],
        &mut remaining,
        clear_input_fields,
    )
    .unwrap();
    assert_eq!(retained["extension"], REDACTED);
    assert!(retained.get("state").is_none());
    let charged = REDACTION_LIMITS.work - remaining;
    assert!(charged > "ONLY_DISCARDED_QZX".len());
    let mut remaining = charged - 1;
    let mut value = original;
    assert!(
        privacy_with_input_filter(
            &mut value,
            &options,
            &[],
            &mut remaining,
            clear_input_fields
        )
        .unwrap_err()
        .is::<ImportLimitError>()
    );
}

fn inputs(prefix: &str) -> Value {
    Value::Object(
        INPUT_FIELDS
            .into_iter()
            .map(|field| {
                (
                    field.into(),
                    json!(format!("PRIVATE_{prefix}_{}", field.to_uppercase())),
                )
            })
            .collect(),
    )
}

fn bodies() -> (Value, Value) {
    let mut request = inputs("REQUEST");
    request["questions"] = json!({"q":{
        "type":"noul", "instructions":"Evaluate", "extension":{
            "state":"SEMANTIC_RULE", "input":"SEMANTIC_RULE_INPUT",
            "image":"SEMANTIC_RULE_MEDIA"
        }
    }});
    request["extension"] = json!({"state":"ORDINARY_REQUEST", "input":"ORDINARY_INPUT"});
    let mut response = inputs("RESPONSE");
    response["original_metadata"] = inputs("LEGACY");
    response["answers"] = json!({"q":{
        "type":"noul", "noul":0.5, "extension":{
            "state":"SEMANTIC_ANSWER", "image":"SEMANTIC_ANSWER_MEDIA"
        }
    }});
    response["extension"] = json!({"original_metadata":{"state":"ORDINARY_RESPONSE"}});
    (request, response)
}

fn capture(request: &Value, response: &Value) -> Capture {
    Capture {
        id: "SYNTHETIC_ID".into(),
        source: "SYNTHETIC_SOURCE".into(),
        timestamp: 1_700_000_000_000,
        status: 200,
        capture_complete: true,
        request: serde_json::to_vec(request).unwrap(),
        response: serde_json::to_vec(response).unwrap(),
        ..Default::default()
    }
}

fn check_bodies(record: &Value, retained: bool) {
    let text = record.to_string();
    for prefix in ["REQUEST", "RESPONSE", "LEGACY"] {
        for field in INPUT_FIELDS {
            assert_eq!(
                text.contains(&format!("PRIVATE_{prefix}_{}", field.to_uppercase())),
                retained,
                "input retention differs at {prefix}.{field}"
            );
        }
    }
    for marker in [
        "SEMANTIC_RULE",
        "SEMANTIC_RULE_INPUT",
        "SEMANTIC_RULE_MEDIA",
        "SEMANTIC_ANSWER",
        "SEMANTIC_ANSWER_MEDIA",
        "ORDINARY_REQUEST",
        "ORDINARY_INPUT",
        "ORDINARY_RESPONSE",
    ] {
        assert!(text.contains(marker), "semantic extension was lost");
    }
}

#[test]
fn input_opt_out_precedes_live_and_raw_capture_key_redaction() {
    for secret in ["state", "image", "audio", "video", "original_metadata", "a"] {
        for capture_state in [false, true] {
            let options = NormalizeOptions {
                capture_state,
                ..Default::default()
            };
            let (mut request, response) = bodies();
            request["credentials"] = json!(secret);
            let capture = capture(&request, &response);
            let before = (capture.request.clone(), capture.response.clone());
            check_bodies(&normalize(&capture, &options), capture_state);
            assert_eq!(
                (&capture.request, &capture.response),
                (&before.0, &before.1)
            );
            let record = json!({
                "id":capture.id, "source":capture.source, "provider":"typesafe",
                "timestamp":capture.timestamp, "status":200, "duration_ms":1, "capture_complete":true,
                "request":request, "response":response
            });
            let imported =
                import_records(&record.to_string(), "systemone-capture", &options).unwrap();
            check_bodies(&imported[0], capture_state);
        }
    }
}

fn receipt() -> Value {
    let mut receipt: Value = serde_json::from_str(include_str!(
        "../../../fixtures/model/jevrouter-receipt.json"
    ))
    .unwrap();
    for key in [
        "routing_input",
        "request",
        "state",
        "input",
        "context",
        "actor",
    ] {
        receipt[key] = json!(format!("PRIVATE_ALIAS_{}", key.to_uppercase()));
    }
    receipt["decision"]["input"] = json!("PRIVATE_DECISION");
    receipt["decision"]["extension"] = json!({"input":"SEMANTIC_DECISION"});
    let (_, response) = bodies();
    receipt["raw_jev"] = response.clone();
    receipt["raw_jev_stages"] = json!([
        {"response":response, "extension":{"state":"ORDINARY_STAGE"}},
        {"response":response}
    ]);
    receipt["extension"] =
        json!({"state":"ORDINARY_RECEIPT", "raw_jev":{"state":"ORDINARY_NESTED"}});
    receipt
}

fn check_receipt(record: &Value, retained: bool) {
    let text = record.to_string();
    for key in [
        "routing_input",
        "request",
        "state",
        "input",
        "context",
        "actor",
    ] {
        assert_eq!(
            text.contains(&format!("PRIVATE_ALIAS_{}", key.to_uppercase())),
            retained,
            "receipt alias retention differs at {key}"
        );
    }
    assert_eq!(text.contains("PRIVATE_DECISION"), retained);
    for prefix in ["RESPONSE", "LEGACY"] {
        for field in INPUT_FIELDS {
            assert_eq!(
                text.contains(&format!("PRIVATE_{prefix}_{}", field.to_uppercase())),
                retained,
                "receipt input retention differs at {prefix}.{field}"
            );
        }
    }
    for marker in [
        "SEMANTIC_DECISION",
        "SEMANTIC_ANSWER",
        "ORDINARY_STAGE",
        "ORDINARY_RECEIPT",
        "ORDINARY_NESTED",
    ] {
        assert!(text.contains(marker), "receipt semantic extension was lost");
    }
}

#[test]
fn input_opt_out_precedes_receipt_leaf_and_ancestor_key_redaction() {
    for secret in [
        "state",
        "image",
        "input",
        "context",
        "actor",
        "decision",
        "raw_jev",
        "raw_jev_stages",
        "response",
        "original_metadata",
    ] {
        for capture_state in [false, true] {
            let mut original = receipt();
            original["credentials"] = json!(secret);
            let result = import_records(
                &original.to_string(),
                "jevrouter-receipt",
                &NormalizeOptions {
                    capture_state,
                    ..Default::default()
                },
            );
            // A renamed required decision object is rejected by format validation;
            // its rejection remains unchanged by input pruning.
            if secret == "decision" {
                assert!(result.is_err());
                continue;
            }
            check_receipt(&result.unwrap()[0], capture_state);
        }
    }
}

#[test]
fn input_opt_out_precedes_observer_extension_and_provenance_key_redaction() {
    for secret in [
        "image",
        "audio",
        "video",
        "original_metadata",
        "request_extra",
        "response_extra",
        "raw_jev",
        "raw_jev_stages",
    ] {
        for capture_state in [false, true] {
            let (request, response) = bodies();
            let mut original = normalize(
                &capture(&request, &response),
                &NormalizeOptions {
                    capture_state: true,
                    ..Default::default()
                },
            );
            original["credentials"] = json!(secret);
            original["receipt"] = receipt();
            original["actions"] = json!([receipt(), receipt()]);
            let imported = import_records(
                &original.to_string(),
                "observer-jsonl",
                &NormalizeOptions {
                    capture_state,
                    ..Default::default()
                },
            )
            .unwrap();
            let record = &imported[0];
            // Renamed optional extension wrappers are not reconstructed, but
            // retained provenance still exercises every input path.
            if !matches!(secret, "request_extra" | "response_extra") {
                check_bodies(record, capture_state);
            }
            check_receipt(record, capture_state);
        }
    }
}

#[test]
fn input_opt_out_keeps_original_discovery_order_and_explicit_priority() {
    for capture_state in [false, true] {
        for explicit in [None, Some("abc")] {
            let request = json!({
                "questions":{}, "state":{"password":"bcd", "token":"ONLY_DISCARDED_QZX"},
                "z_extension":{"password":"abc", "echo":"abcd", "discarded_echo":"ONLY_DISCARDED_QZX"}
            });
            let mut capture = capture(&request, &json!({}));
            capture.secret = explicit.map(str::to_owned);
            let record = normalize(
                &capture,
                &NormalizeOptions {
                    capture_state,
                    ..Default::default()
                },
            );
            assert_eq!(
                record["request_extra"]["z_extension"]["echo"],
                if explicit.is_some() {
                    "[REDACTED]d"
                } else {
                    "a[REDACTED]"
                }
            );
            assert_eq!(
                record["request_extra"]["z_extension"]["discarded_echo"],
                REDACTED
            );
        }
    }
}

#[test]
fn input_opt_out_state_secret_preserves_distinct_capture_identities() {
    let request = json!({
        "state":{"password":"state", "snapshot":"PRIVATE_INPUT_QZX"},
        "questions":{}, "extension":{"echo":"state", "keep":"ORDINARY_QZX"}
    });
    let response = json!({"answers":{}});
    let options = NormalizeOptions::default();
    let mut imports = Vec::new();
    for id in ["SYNTHETIC_ONE", "SYNTHETIC_TWO"] {
        let mut capture = capture(&request, &response);
        capture.id = id.into();
        let live = normalize(&capture, &options);
        assert_eq!(live["id"], id);
        assert_eq!(live["source"], "SYNTHETIC_SOURCE");
        assert_eq!(live["capture_complete"], true);
        assert_eq!(live["request_extra"]["extension"]["echo"], REDACTED);
        assert!(!live.to_string().contains("PRIVATE_INPUT_QZX"));
        imports.push(json!({
            "id":id, "source":"SYNTHETIC_SOURCE", "provider":"typesafe",
            "timestamp":capture.timestamp, "status":200, "duration_ms":1,
            "capture_complete":true, "request":request, "response":response
        }));
    }
    let text = imports
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let records = import_records(&text, "systemone-capture", &options).unwrap();
    assert_eq!(records.len(), 2);
    assert_ne!(records[0]["id"], records[1]["id"]);
    for record in records {
        assert!(!record["id"].as_str().unwrap().is_empty());
        assert_eq!(record["source"], "SYNTHETIC_SOURCE");
        assert_eq!(record["capture_complete"], true);
        assert_eq!(record["request_extra"]["extension"]["echo"], REDACTED);
        assert!(!record.to_string().contains("PRIVATE_INPUT_QZX"));
    }
}

#[test]
fn input_opt_out_omits_discarded_envelope_keys_before_they_can_collide() {
    let request = json!({
        "state":"PRIVATE_STATE_QZX", "images":"PRIVATE_MEDIA_QZX", "questions":{},
        "credentials":["state", "images"], "extension":"ORDINARY_QZX"
    });
    let response = json!({"state":"PRIVATE_STATE_QZX", "images":"PRIVATE_MEDIA_QZX"});
    let capture = capture(&request, &response);
    let options = NormalizeOptions::default();
    let live = normalize(&capture, &options);
    let input = json!({
        "id":"SYNTHETIC_ID", "source":"SYNTHETIC_SOURCE", "provider":"typesafe",
        "timestamp":capture.timestamp, "status":200, "duration_ms":1,
        "capture_complete":true, "request":request, "response":response
    });
    let imported = import_records(&input.to_string(), "systemone-capture", &options).unwrap();
    for record in [&live, &imported[0]] {
        assert_eq!(record["capture_complete"], true);
        assert_eq!(record["request_extra"]["extension"], "ORDINARY_QZX");
        for key in ["request_extra", "response_extra"] {
            // Null placeholders would still collide after both names redact.
            assert!(record[key].get(REDACTED).is_none());
        }
        assert!(!record.to_string().contains("PRIVATE_"));
    }
}

#[test]
fn input_opt_out_discovers_discarded_only_secrets_in_all_import_formats() {
    for format in ["systemone-capture", "observer-jsonl", "jevrouter-receipt"] {
        let (mut request, response) = bodies();
        request["state"] = json!({"password":"ONLY_DISCARDED_QZX"});
        request["questions"]["q"]["instructions"] = json!("ONLY_DISCARDED_QZX");
        let original = match format {
            "systemone-capture" => {
                json!({"id":"SYNTHETIC_ID","source":"SYNTHETIC_SOURCE","provider":"typesafe","timestamp":1_700_000_000_000_i64,"status":200,"duration_ms":1,"capture_complete":true,"request":request,"response":response})
            }
            "observer-jsonl" => {
                // Inject after export creation so import must discover the secret.
                let mut record = normalize(
                    &capture(
                        &json!({"questions":{"q":{"type":"noul","instructions":"ONLY_DISCARDED_QZX"}}}),
                        &response,
                    ),
                    &NormalizeOptions::default(),
                );
                record["state"] = request["state"].clone();
                record["state_retained"] = json!(true);
                record
            }
            _ => {
                let mut record = receipt();
                record["input"] = request["state"].clone();
                record["extension"]["echo"] = json!("ONLY_DISCARDED_QZX");
                record
            }
        };
        let imported =
            import_records(&original.to_string(), format, &NormalizeOptions::default()).unwrap();
        assert!(
            !imported[0].to_string().contains("ONLY_DISCARDED_QZX"),
            "discarded-only credential leaked for {format}"
        );
        assert!(imported[0].to_string().contains(REDACTED));
    }
}
