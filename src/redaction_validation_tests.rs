use super::*;

fn capture(provider: &str, definition: Value, answer: Value) -> (Capture, NormalizeOptions) {
    (
        Capture {
            id: CaptureId::new_live(),
            timestamp: 1_800_000_000_000,
            source: "fixture".into(),
            status: 200,
            duration_ms: 12.5,
            capture_complete: true,
            request: serde_json::to_vec(&json!({"questions":{"q":definition}})).unwrap(),
            response: serde_json::to_vec(&json!({"answers":{"q":answer}})).unwrap(),
            ..Default::default()
        },
        NormalizeOptions {
            provider: Some(provider.into()),
            ..Default::default()
        },
    )
}

fn raw(item: &Capture, options: &NormalizeOptions) -> Value {
    json!({"id":"fixture-event", "source":"fixture", "provider":options.provider,
        "timestamp":item.timestamp, "status":item.status, "capture_complete":true,
        "request":serde_json::from_slice::<Value>(&item.request).unwrap(),
        "response":serde_json::from_slice::<Value>(&item.response).unwrap()})
}

fn rejected(error: anyhow::Error) {
    assert!(error.is::<RedactionValidationError>(), "{error:#}");
    assert_eq!(
        error
            .downcast_ref::<RedactionValidationError>()
            .unwrap()
            .to_string(),
        IMPORT_REDACTION_VALIDATION_MESSAGE
    );
}

fn incomplete(item: &Capture, options: &NormalizeOptions) -> Value {
    let record = normalize(item, options);
    assert_eq!(record["capture_complete"], false);
    assert_eq!(record["normalization_error"], REDACTION_VALIDATION_MESSAGE);
    assert_eq!(record["answers"], json!([]));
    assert_eq!(record["source"], REDACTED);
    assert_eq!(record["timestamp"], item.timestamp);
    assert_eq!(record["status"], item.status);
    assert_eq!(record["duration_ms"], item.duration_ms);
    assert!(record.to_string().len() < 2048);
    record
}

type ValidationCase = (&'static str, &'static str, bool, Value, Value, Value);

fn cases() -> Vec<ValidationCase> {
    let noul = json!({"type":"noul","instructions":"Apply the rule"});
    let choice = json!({"type":"choice","instructions":"Pick","criteria":{"a":null,"b":null}});
    let choice_answer =
        json!({"type":"choice","choice":"a","probabilities":{"a":0.8,"b":0.2},"confidence":0.7});
    let score = json!({"type":"score","instructions":"Rate","criteria":["Low","High"]});
    let score_answer = json!({"type":"score","score":0.5,"probabilities":{"0":0.5,"1":0.5},"confidence":0.7,"legend":{"0":"Low","1":"High"}});
    vec![
        (
            "typesafe",
            "criteria",
            true,
            noul.clone(),
            json!({"type":"noul","noul":0.2}),
            json!({"invalid":"Wrong criterion"}),
        ),
        (
            "decider",
            "instructions",
            true,
            noul,
            json!({"type":"noul","noul":0.2}),
            json!(12),
        ),
        (
            "semif",
            "confidence",
            false,
            choice,
            choice_answer,
            json!(2),
        ),
        (
            "semif",
            "confidence",
            false,
            score.clone(),
            score_answer.clone(),
            json!(2),
        ),
        (
            "semif",
            "legend",
            false,
            score,
            score_answer,
            json!({"0":"Low"}),
        ),
    ]
}

#[test]
fn optional_validation_key_redaction_cannot_make_invalid_answers_valid() {
    for (provider, field, definition_field, definition, answer, bad) in cases() {
        let (mut item, mut options) = capture(provider, definition, answer);
        let mut request: Value = serde_json::from_slice(&item.request).unwrap();
        let mut response: Value = serde_json::from_slice(&item.response).unwrap();
        if definition_field {
            request["questions"]["q"][field] = bad;
        } else {
            response["answers"]["q"][field] = bad;
        }
        item.request = serde_json::to_vec(&request).unwrap();
        item.response = serde_json::to_vec(&response).unwrap();
        let original = normalize(&item, &options);
        assert_eq!(original["answers"][0]["valid"], false, "{provider}/{field}");

        request["credentials"] = json!([field]);
        item.request = serde_json::to_vec(&request).unwrap();
        for capture_state in [false, true] {
            options.capture_state = capture_state;
            let first = incomplete(&item, &options);
            assert_ne!(first["id"], incomplete(&item, &options)["id"]);
            rejected(
                import_records(
                    &raw(&item, &options).to_string(),
                    "systemone-capture",
                    &options,
                )
                .unwrap_err(),
            );
            let mut observer = original.clone();
            observer["request_extra"]["credentials"] = json!([field]);
            rejected(
                import_records(&observer.to_string(), "observer-jsonl", &options).unwrap_err(),
            );
            let restored = import_records(&first.to_string(), "observer-jsonl", &options).unwrap();
            for key in ["id", "answers", "capture_complete", "normalization_error"] {
                assert_eq!(restored[0][key], first[key]);
            }
        }
    }
}

#[test]
fn genuinely_absent_optional_fields_and_arbitrary_metadata_remain_valid() {
    for (provider, field, definition_field, definition, answer, _) in cases() {
        let (mut item, mut options) = capture(provider, definition, answer);
        let mut request: Value = serde_json::from_slice(&item.request).unwrap();
        let mut response: Value = serde_json::from_slice(&item.response).unwrap();
        if definition_field {
            request["questions"]["q"]
                .as_object_mut()
                .unwrap()
                .remove(field);
        } else {
            response["answers"]["q"]
                .as_object_mut()
                .unwrap()
                .remove(field);
        }
        request["credentials"] = json!([field]);
        // These look like protocol objects but are ordinary user metadata.
        let metadata = json!({"questions":{"q":{"type":"noul",field:"private-echo"}},"answers":{"q":{"type":"score",field:"private-echo"}}});
        request["extension"] = metadata.clone();
        request["state"] = metadata.clone();
        request["questions"]["q"]["extension"] = metadata.clone();
        response["answers"]["q"]["extension"] = metadata;
        item.request = serde_json::to_vec(&request).unwrap();
        item.response = serde_json::to_vec(&response).unwrap();
        for capture_state in [false, true] {
            options.capture_state = capture_state;
            let record = normalize(&item, &options);
            assert_eq!(record["capture_complete"], true, "{provider}/{field}");
            assert_eq!(
                record["answers"][0]["valid"], true,
                "{provider}/{field}: {record}"
            );
            assert!(record["normalization_error"].is_null());
            assert_eq!(record["state_retained"], capture_state);
            assert_eq!(
                record["request_extra"]["extension"]["questions"]["q"][REDACTED],
                "private-echo"
            );
            let imported = import_records(
                &raw(&item, &options).to_string(),
                "systemone-capture",
                &options,
            )
            .unwrap();
            assert_eq!(imported[0]["answers"][0]["valid"], true);
        }
    }
}

#[test]
fn required_field_redaction_remains_explicitly_invalid_without_new_optional_failure() {
    for field in ["questions", "answers", "type", "noul"] {
        let (mut item, options) = capture(
            "typesafe",
            json!({"type":"noul","instructions":"Rule"}),
            json!({"type":"noul","noul":0.2}),
        );
        item.secret = Some(field.into());
        let record = normalize(&item, &options);
        assert_eq!(record["capture_complete"], true);
        assert_eq!(record["answers"][0]["valid"], false);
        assert!(record["answers"][0]["error"].is_string());
        if matches!(field, "questions" | "answers") {
            assert!(record["normalization_error"].is_string());
        }
    }
}

#[test]
fn observer_provider_redaction_cannot_substitute_a_different_validator() {
    let (item, options) = capture(
        "eikos",
        json!({"type":"score","instructions":"Rate","criteria":["Low","High"]}),
        json!({"type":"score","score":0.5,"probabilities":{"0":0.5,"1":0.5},"confidence":0.7,"legend":{"0":"Low","1":"High"}}),
    );
    let mut original = normalize(&item, &options);
    assert_eq!(original["answers"][0]["valid"], false);
    let mut raw_record = raw(&item, &options);
    raw_record["request"]["credentials"] = json!(["eikos"]);
    let raw_imported = import_records(&raw_record.to_string(), "systemone-capture", &options)
        .unwrap()
        .remove(0);
    assert_eq!(raw_imported["provider"], "eikos");
    assert_eq!(raw_imported["answers"][0]["valid"], false);
    assert_eq!(
        import_records(&raw_imported.to_string(), "observer-jsonl", &options).unwrap()[0]["answers"]
            [0]["valid"],
        false
    );
    for secret in ["eikos", "eik"] {
        original["request_extra"]["credentials"] = json!([secret]);
        rejected(import_records(&original.to_string(), "observer-jsonl", &options).unwrap_err());
    }
    // Unrelated redaction preserves modal invalidity. A genuinely unknown
    // provider keeps its default profile even if its label is redacted.
    original["request_extra"]["credentials"] = json!(["unrelated-private-value"]);
    assert_eq!(
        import_records(&original.to_string(), "observer-jsonl", &options).unwrap()[0]["answers"][0]
            ["valid"],
        false
    );
    original["provider"] = json!("private-custom-provider");
    original["request_extra"]["credentials"] = json!(["private-custom-provider"]);
    assert_eq!(
        import_records(&original.to_string(), "observer-jsonl", &options).unwrap()[0]["answers"][0]
            ["valid"],
        true
    );
}

#[test]
fn receipt_provenance_is_not_interpreted_as_inference_protocol() {
    let mut receipt: Value =
        serde_json::from_str(include_str!("../fixtures/model/jevrouter-receipt.json")).unwrap();
    receipt["credentials"] = json!(["criteria"]);
    receipt["raw_jev"]["questions"] = json!({"q":{"type":"noul","criteria":{"invalid":"rule"}}});
    let options = NormalizeOptions::default();
    let record = import_records(&receipt.to_string(), "jevrouter-receipt", &options)
        .unwrap()
        .remove(0);
    assert_eq!(record["event_kind"], "application_action");
    assert_eq!(record["answers"], json!([]));
    assert!(
        record["receipt"]["raw_jev"]["questions"]["q"]
            .get(REDACTED)
            .is_some()
    );
    let restored = import_records(&record.to_string(), "observer-jsonl", &options).unwrap();
    assert_eq!(restored[0]["answers"], json!([]));
}

#[test]
fn semantic_redaction_failure_keeps_shared_work_charges() {
    let (item, options) = capture(
        "typesafe",
        json!({"type":"noul","instructions":"Rule","criteria":{"invalid":"rule"}}),
        json!({"type":"noul","noul":0.2}),
    );
    let mut record = normalize(&item, &options);
    record["request_extra"]["credentials"] = json!(["criteria"]);
    let mut remaining = REDACTION_LIMITS.work;
    rejected(privacy_observer(&record, &options, &mut remaining).unwrap_err());
    assert!(remaining < REDACTION_LIMITS.work);
    remaining = REDACTION_LIMITS.work - remaining - 1;
    assert!(
        privacy_observer(&record, &options, &mut remaining)
            .unwrap_err()
            .is::<ImportLimitError>()
    );
}
