use super::*;

fn observer() -> Value {
    normalize(
        &Capture {
            id: "stable-parent".into(),
            source: "fixture-source".into(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            status: 200,
            capture_complete: true,
            request: br#"{"questions":{}}"#.to_vec(),
            response: br#"{"answers":{}}"#.to_vec(),
            ..Default::default()
        },
        &NormalizeOptions::default(),
    )
}

fn receipt() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/model/jevrouter-receipt.json"
    ))
    .unwrap()
}

fn rejected(text: &str, format: &str, options: &NormalizeOptions) {
    let failure =
        import_records(text, format, options).expect_err("redacted identity must be rejected");
    let explanation = format!("{failure:#}");
    assert!(
        explanation.contains("privacy filtering changes an event identity"),
        "unexpected error for {format}, private keys {:?}: {explanation}",
        options.redact_keys
    );
    assert!(!explanation.contains("private-identity"));
    assert!(!explanation.contains("fixture-source"));
}

#[test]
fn observer_import_identity_redaction_rejects_distinct_events_before_they_merge() {
    for field in ["id", "source", "source_event_id"] {
        let records: Vec<_> = ["X", "Y"]
            .into_iter()
            .enumerate()
            .map(|(index, secret)| {
                let mut record = observer();
                record["id"] = json!(format!("parent-{index}"));
                record["source_event_id"] = if field == "source" {
                    json!("same-event-in-distinct-sources")
                } else {
                    Value::Null
                };
                record[field] = json!(format!("private-identity-{secret}"));
                record["request_extra"] = json!({"credentials":secret});
                record
            })
            .collect();
        rejected(
            &format!("{}\n{}", records[0], records[1]),
            "observer-jsonl",
            &NormalizeOptions::default(),
        );
    }
}

#[test]
fn receipt_import_identity_redaction_rejects_decisions_but_allows_request_provenance() {
    let mut first = receipt();
    first["decision_id"] = json!("private-identity-X");
    first["credentials"] = json!("X");
    let mut second = first.clone();
    second["decision_id"] = json!("private-identity-Y");
    second["credentials"] = json!("Y");
    rejected(
        &json!([first, second]).to_string(),
        "jevrouter-receipt",
        &NormalizeOptions::default(),
    );

    let clean = receipt();
    let expected = import_records(
        &clean.to_string(),
        "jevrouter-receipt",
        &NormalizeOptions::default(),
    )
    .unwrap();
    let mut record = clean;
    record["request_id"] = json!("private-request-X");
    record["credentials"] = json!("X");
    let imported = import_records(
        &record.to_string(),
        "jevrouter-receipt",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(imported[0]["id"], expected[0]["id"]);
    assert_eq!(
        imported[0]["source_event_id"],
        expected[0]["source_event_id"]
    );
    assert_eq!(
        imported[0]["receipt"]["request_id"],
        "private-request-[REDACTED]"
    );
}

#[test]
fn import_identity_fallbacks_and_existing_redacted_exports_remain_stable() {
    for event in [Value::Null, json!(""), json!("retained-event-[REDACTED]")] {
        let mut record = observer();
        record["id"] = json!("retained-parent-[REDACTED]");
        record["source"] = json!("retained-source-[REDACTED]");
        record["source_event_id"] = event.clone();
        record["request_extra"] = json!({"credentials":"X", "echo":"X"});
        let expected_event = if event.as_str().is_some_and(|value| !value.is_empty()) {
            event
        } else {
            record["id"].clone()
        };
        let first = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        let again = import_records(
            &first[0].to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        for imported in [&first[0], &again[0]] {
            assert_eq!(imported["id"], record["id"]);
            assert_eq!(imported["source"], record["source"]);
            assert_eq!(imported["source_event_id"], expected_event);
            assert_eq!(imported["request_extra"]["echo"], REDACTED);
        }
    }
}

#[test]
fn import_identity_redaction_in_reconstructed_envelopes_is_rejected() {
    let mut record = observer();
    record["request_extra"] = json!({"echo":"stable-parent"});
    rejected(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions {
            redact_keys: vec!["request".into()],
            ..Default::default()
        },
    );
    for key in ["id", "source", "import_format"] {
        let mut record = receipt();
        // Keep this fixture focused on reconstructed identity. With private
        // candidate IDs, the original probability keys also collide and are
        // correctly rejected by the earlier object-key collision guard.
        record["raw_jev"]["answers"] = json!({});
        rejected(
            &record.to_string(),
            "jevrouter-receipt",
            &NormalizeOptions {
                redact_keys: vec![key.into()],
                ..Default::default()
            },
        );
    }
}

#[test]
fn import_identity_prefixes_and_configured_keys_do_not_bypass_privacy() {
    let mut record = observer();
    record["id"] = json!("capture_v1_private-identity-X");
    record["request_extra"] = json!({"credentials":"X"});
    rejected(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    );
    for key in ["id", "source", "source_event_id", "import_format"] {
        rejected(
            &observer().to_string(),
            "observer-jsonl",
            &NormalizeOptions {
                redact_keys: vec![key.into()],
                ..Default::default()
            },
        );
    }
}
