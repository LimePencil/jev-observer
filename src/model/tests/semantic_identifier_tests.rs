use super::*;

fn capture(criteria: Value, selected: &str, probability_key: &str) -> Capture {
    Capture {
        id: CaptureId::new_live(),
        timestamp: 1_800_000_000_000,
        source: "fixture".into(),
        status: 200,
        duration_ms: 12.5,
        capture_complete: true,
        request: serde_json::to_vec(&json!({"questions":{"q":{
            "type":"choice", "instructions":"Choose an option", "criteria":criteria
        }}})).unwrap(),
        response: serde_json::to_vec(&json!({"answers":{"q":{
            "type":"choice", "choice":selected, "probabilities":{probability_key:1}, "confidence":0.7
        }}})).unwrap(),
        ..Default::default()
    }
}

fn secrets(item: &mut Capture, values: &[&str]) {
    let mut request: Value = serde_json::from_slice(&item.request).unwrap();
    request["credentials"] = json!(values);
    item.request = serde_json::to_vec(&request).unwrap();
}

fn raw(item: &Capture, options: &NormalizeOptions) -> Value {
    json!({"id":"fixture-event", "source":"fixture", "provider":options.provider.as_deref().unwrap_or("typesafe"),
        "timestamp":item.timestamp, "status":item.status, "capture_complete":true,
        "request":serde_json::from_slice::<Value>(&item.request).unwrap(),
        "response":serde_json::from_slice::<Value>(&item.response).unwrap()})
}

fn collision(error: anyhow::Error) {
    assert!(error.is::<RedactionCollisionError>(), "{error:#}");
    assert_eq!(
        error
            .downcast_ref::<RedactionCollisionError>()
            .unwrap()
            .to_string(),
        IMPORT_REDACTION_COLLISION_MESSAGE
    );
}

fn incomplete(item: &Capture, options: &NormalizeOptions) -> Value {
    let record = normalize(item, options);
    assert_eq!(record["capture_complete"], false);
    assert_eq!(record["normalization_error"], REDACTION_COLLISION_MESSAGE);
    assert_eq!(record["answers"], json!([]));
    assert_eq!(record["source"], REDACTED);
    for (field, expected) in [
        ("timestamp", json!(item.timestamp)),
        ("status", json!(item.status)),
        ("duration_ms", json!(item.duration_ms)),
    ] {
        assert_eq!(record[field], expected);
    }
    assert!(!record.to_string().contains("option-"));
    assert!(record.to_string().len() < 2048);
    record
}

#[test]
fn semantic_identifier_aliases_cannot_validate_mismatched_choice_fields() {
    for (provider, list) in [("typesafe", false), ("laya", true)] {
        let options = NormalizeOptions {
            provider: Some(provider.into()),
            ..Default::default()
        };
        for (criterion, selected, probability_key, known) in [
            ("option-X", "option-Y", "option-Y", vec!["X", "Y"]),
            ("option-X", "option-X", "option-Y", vec!["X", "Y"]),
            ("option-X", "option-Y", "option-X", vec!["X", "Y"]),
            (
                "option-X",
                "option-[REDACTED]",
                "option-[REDACTED]",
                vec!["X"],
            ),
        ] {
            let criteria = if list {
                json!([criterion])
            } else {
                json!({criterion:null})
            };
            let mut item = capture(criteria, selected, probability_key);
            let original = normalize(&item, &options);
            assert_eq!(original["answers"][0]["valid"], false);
            let error = original["answers"][0]["error"].as_str().unwrap();
            assert!(
                error.contains("Selected option is absent")
                    || error.contains("Probability keys do not match")
            );
            secrets(&mut item, &["unrelated-private-credential"]);
            let unrelated = normalize(&item, &options);
            assert_eq!(
                unrelated["answers"][0]["error"],
                original["answers"][0]["error"]
            );
            secrets(&mut item, &known);
            let first = incomplete(&item, &options);
            assert_ne!(first["id"], incomplete(&item, &options)["id"]);
            collision(
                import_records(
                    &raw(&item, &options).to_string(),
                    "systemone-capture",
                    &options,
                )
                .unwrap_err(),
            );
            let mut observer = original;
            observer["request_extra"]["credentials"] = json!(known);
            collision(
                import_records(&observer.to_string(), "observer-jsonl", &options).unwrap_err(),
            );
            let restored = import_records(&first.to_string(), "observer-jsonl", &options).unwrap();
            for field in ["id", "capture_complete", "normalization_error", "answers"] {
                assert_eq!(restored[0][field], first[field]);
            }
        }
    }
}

#[test]
fn semantic_identifier_aliases_cannot_pair_distinct_question_and_answer_keys() {
    let options = NormalizeOptions::default();
    let mut item = capture(json!({"yes":null}), "yes", "yes");
    let mut request: Value = serde_json::from_slice(&item.request).unwrap();
    let mut response: Value = serde_json::from_slice(&item.response).unwrap();
    request["questions"] = json!({"question-X": request["questions"]["q"]});
    response["answers"] = json!({"question-Y":response["answers"]["q"]});
    item.request = serde_json::to_vec(&request).unwrap();
    item.response = serde_json::to_vec(&response).unwrap();
    let original = normalize(&item, &options);
    assert_eq!(original["answers"].as_array().unwrap().len(), 2);
    assert!(
        original["answers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|answer| answer["valid"] == false)
    );
    secrets(&mut item, &["X", "Y"]);
    incomplete(&item, &options);
    collision(
        import_records(
            &raw(&item, &options).to_string(),
            "systemone-capture",
            &options,
        )
        .unwrap_err(),
    );
    let mut observer = original;
    observer["request_extra"]["credentials"] = json!(["X", "Y"]);
    collision(import_records(&observer.to_string(), "observer-jsonl", &options).unwrap_err());
}

#[test]
fn semantic_identifier_aliases_cannot_reattach_or_merge_imported_reviews() {
    let options = NormalizeOptions::default();
    for (answer_key, label_keys, known) in [
        ("q", vec!["q-X", "q-Y"], vec!["X", "Y"]),
        ("q-X", vec!["q-Y"], vec!["X", "Y"]),
        ("q-X", vec!["q-[REDACTED]"], vec!["X"]),
        ("q-[REDACTED]", vec!["q-X"], vec!["X"]),
    ] {
        let mut record = normalize(&capture(json!({"yes":null}), "yes", "yes"), &options);
        record["answers"][0]["key"] = json!(answer_key);
        record["labels"] = json!(
            label_keys
                .iter()
                .map(|key| json!({
                    "key":key,"label":"incorrect","reason":"Review of its original question"
                }))
                .collect::<Vec<_>>()
        );
        // An unrelated or missing question's review is permitted as provenance
        // until redaction would make its identity indistinguishable.
        let unchanged = import_records(&record.to_string(), "observer-jsonl", &options).unwrap();
        assert_eq!(
            unchanged[0]["labels"].as_array().unwrap().len(),
            label_keys.len()
        );
        record["request_extra"]["credentials"] = json!(known);
        collision(import_records(&record.to_string(), "observer-jsonl", &options).unwrap_err());
    }
}

#[test]
fn semantic_identifier_imported_reviews_keep_matching_keys_and_provenance() {
    let options = NormalizeOptions::default();
    let mut record = normalize(&capture(json!({"yes":null}), "yes", "yes"), &options);
    record["answers"][0]["key"] = json!("q-X");
    record["request_extra"]["credentials"] = json!(["X"]);
    record["labels"] = json!([
        {"key":"q-X","label":"correct","reason":"First assessment","source":"reviewer"},
        {"key":"q-X","label":"incorrect","reason":"Updated assessment","source":"reviewer"}
    ]);
    let imported = import_records(&record.to_string(), "observer-jsonl", &options).unwrap();
    let again = import_records(&imported[0].to_string(), "observer-jsonl", &options).unwrap();
    assert_eq!(imported[0]["answers"][0]["key"], "q-[REDACTED]");
    for (index, label) in imported[0]["labels"].as_array().unwrap().iter().enumerate() {
        assert_eq!(label["key"], imported[0]["answers"][0]["key"]);
        for field in ["label", "reason", "source"] {
            assert_eq!(label[field], record["labels"][index][field]);
        }
    }
    assert_eq!(again[0]["labels"], imported[0]["labels"]);
}

#[test]
fn semantic_identifier_private_review_keys_cannot_repair_invalid_types() {
    let options = NormalizeOptions {
        redact_keys: vec!["key".into()],
        ..Default::default()
    };
    for key in [Value::Null, json!(false), json!(1), json!([]), json!({})] {
        let mut record = normalize(
            &capture(json!({"yes":null}), "yes", "yes"),
            &NormalizeOptions::default(),
        );
        record["answers"][0]["key"] = json!("opaque-question");
        record["labels"] = json!([{"key":key,"label":"correct"}]);
        let error = import_records(&record.to_string(), "observer-jsonl", &options).unwrap_err();
        assert!(error.is::<RedactionValidationError>(), "{error:#}");
    }
}

#[test]
fn semantic_identifier_repeated_originals_and_separate_questions_remain_valid() {
    let options = NormalizeOptions::default();
    let mut item = capture(json!({"option-X":null}), "option-X", "option-X");
    let other = capture(json!({"option-Y":null}), "option-Y", "option-Y");
    let mut request: Value = serde_json::from_slice(&item.request).unwrap();
    let mut response: Value = serde_json::from_slice(&item.response).unwrap();
    request["questions"]["other"] =
        serde_json::from_slice::<Value>(&other.request).unwrap()["questions"]["q"].clone();
    response["answers"]["other"] =
        serde_json::from_slice::<Value>(&other.response).unwrap()["answers"]["q"].clone();
    // These fields contain text, not identifiers. Separate metadata objects and
    // described criteria may legitimately contain the same retained text.
    request["extension"] = json!({"first":{"value":"option-X"},"second":{"value":"option-Y"}});
    request["questions"]["q"]["criteria"]["option-X"] = json!("option-Y");
    item.request = serde_json::to_vec(&request).unwrap();
    item.response = serde_json::to_vec(&response).unwrap();
    secrets(&mut item, &["X", "Y"]);
    for _ in 0..2 {
        let output = normalize(&item, &options);
        assert_eq!(output["capture_complete"], true);
        assert_eq!(output["normalization_error"], Value::Null);
        assert_eq!(output["answers"].as_array().unwrap().len(), 2);
        for answer in output["answers"].as_array().unwrap() {
            assert_eq!(answer["valid"], true);
            assert_eq!(answer["value"], "option-[REDACTED]");
            assert_eq!(answer["definition_redacted"], true);
        }
        let raw_output = import_records(
            &raw(&item, &options).to_string(),
            "systemone-capture",
            &options,
        )
        .unwrap();
        let restored = import_records(&output.to_string(), "observer-jsonl", &options).unwrap();
        for records in [&raw_output, &restored] {
            assert!(
                records[0]["answers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|answer| answer["valid"] == true)
            );
        }
    }
    let marker = capture(
        json!({"option-[REDACTED]":null}),
        "option-[REDACTED]",
        "option-[REDACTED]",
    );
    assert_eq!(normalize(&marker, &options)["answers"][0]["valid"], true);
}

#[test]
fn semantic_identifier_supported_lists_keep_equal_originals_and_existing_invalidity() {
    let options = NormalizeOptions {
        provider: Some("laya".into()),
        ..Default::default()
    };
    for (criteria, valid) in [
        (json!(["option-X"]), true),
        (json!(["option-X", "option-X"]), false),
    ] {
        let mut item = capture(criteria, "option-X", "option-X");
        secrets(&mut item, &["X"]);
        let output = normalize(&item, &options);
        assert_eq!(output["capture_complete"], true);
        assert_eq!(output["normalization_error"], Value::Null);
        assert_eq!(output["answers"][0]["valid"], valid);
    }
    // Unsupported list criteria remain invalid; they are not treated as a
    // supported identifier namespace solely because their elements are strings.
    let mut item = capture(json!(["option-X", "option-Y"]), "option-X", "option-X");
    secrets(&mut item, &["X", "Y"]);
    let output = normalize(&item, &NormalizeOptions::default());
    assert_eq!(output["capture_complete"], true);
    assert_eq!(output["answers"][0]["valid"], false);
}

#[test]
fn semantic_identifier_whole_value_redaction_checks_empty_original_strings() {
    // Empty strings are not discovered as secrets, but a custom private field
    // still replaces the selected value with the literal redaction marker.
    let item = capture(json!({"[REDACTED]":null}), "", "[REDACTED]");
    let options = NormalizeOptions {
        redact_keys: vec!["choice".into()],
        ..Default::default()
    };
    incomplete(&item, &options);
    collision(
        import_records(
            &raw(&item, &options).to_string(),
            "systemone-capture",
            &options,
        )
        .unwrap_err(),
    );
}

#[test]
fn semantic_identifier_private_values_cannot_invent_strings_from_invalid_types() {
    let options = NormalizeOptions {
        redact_keys: vec!["choice".into()],
        ..Default::default()
    };
    for value in [Value::Null, json!(false), json!(1), json!([]), json!({})] {
        let mut item = capture(json!({"[REDACTED]":null}), "unused", "[REDACTED]");
        let mut response: Value = serde_json::from_slice(&item.response).unwrap();
        response["answers"]["q"]["choice"] = value;
        item.response = serde_json::to_vec(&response).unwrap();
        let original = normalize(&item, &NormalizeOptions::default());
        assert_eq!(original["answers"][0]["valid"], false);
        assert!(
            original["answers"][0]["error"]
                .as_str()
                .unwrap()
                .contains("Selected option is absent")
        );
        let output = normalize(&item, &options);
        assert_eq!(output["capture_complete"], false);
        assert_eq!(output["normalization_error"], REDACTION_VALIDATION_MESSAGE);
        assert_eq!(output["answers"], json!([]));
        let error = import_records(
            &raw(&item, &options).to_string(),
            "systemone-capture",
            &options,
        )
        .unwrap_err();
        assert!(error.is::<RedactionValidationError>());
    }
    for label in ["original-label", "[REDACTED]"] {
        let item = capture(json!({label:null}), label, label);
        let output = normalize(&item, &options);
        assert_eq!(output["capture_complete"], true);
        assert_eq!(output["answers"][0]["valid"], true);
        assert_eq!(output["answers"][0]["value"], REDACTED);
    }
}

#[test]
fn semantic_identifier_registry_is_charged_before_storage_and_keeps_consumed_work() {
    let mut matcher = SecretMatcher::new(
        vec![],
        RedactionLimits {
            work: 1,
            ..REDACTION_LIMITS
        },
    )
    .unwrap();
    let mut identifiers = SemanticIdentifiers::default();
    assert!(
        identifiers
            .observe(
                IdentifierNamespace::Choice("question"),
                "original",
                "retained",
                &mut matcher
            )
            .unwrap_err()
            .is::<ImportLimitError>()
    );
    assert_eq!(identifiers.count, 0);
    assert!(identifiers.questions.is_empty() && identifiers.choices.is_empty());

    let options = NormalizeOptions::default();
    let mut item = capture(json!({"option-X":null}), "option-Y", "option-Y");
    secrets(&mut item, &["X", "Y"]);
    let mut envelope = raw(&item, &options);
    let mut remaining = REDACTION_LIMITS.work;
    collision(privacy_capture_envelope(&mut envelope, &options, &[], &mut remaining).unwrap_err());
    assert!(remaining < REDACTION_LIMITS.work);
    let after_first = remaining;
    collision(privacy_capture_envelope(&mut envelope, &options, &[], &mut remaining).unwrap_err());
    assert!(remaining < after_first);
}

#[test]
fn semantic_identifier_collision_wording_preserves_old_saved_reasons() {
    let options = NormalizeOptions::default();
    let mut item = capture(json!({"option-X":null}), "option-Y", "option-Y");
    secrets(&mut item, &["X", "Y"]);
    let mut record = incomplete(&item, &options);
    let old = "Credential redaction caused an object key collision; request and response content were not retained";
    record["normalization_error"] = json!(old);
    let restored = import_records(&record.to_string(), "observer-jsonl", &options).unwrap();
    assert_eq!(restored[0]["normalization_error"], old);
    assert_eq!(restored[0]["id"], record["id"]);
    assert_eq!(restored[0]["answers"], json!([]));
}
