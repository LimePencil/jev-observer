use super::*;

fn observer(protocol: Protocol, answers: bool) -> Value {
    let questions = if answers {
        json!({"q":{"type":"noul","instructions":"Inspect"}})
    } else {
        json!({})
    };
    let mut response = json!({"model":"reported-model","answers":if answers {
        json!({"q":{"type":"noul","noul":0.8}})
    } else { json!({}) }});
    if protocol == Protocol::CloudflareSystemone {
        response = json!({"success":false,
            "errors":[{"code":123,"message":"Synthetic upstream failure"}],
            "messages":["Synthetic diagnostic"],"result":response});
    }
    import_records(
        &json!({"id":"envelope-event","source":"fixture","provider":"custom",
            "protocol":protocol,"timestamp":1_800_000_000_000_i64,"status":200,
            "capture_complete":true,"request":{"questions":questions},"response":response})
        .to_string(),
        "systemone-capture",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0)
}

fn restored(record: &Value, options: &NormalizeOptions) -> Value {
    import_records(&record.to_string(), "observer-jsonl", options)
        .unwrap()
        .remove(0)
}

fn privacy_rejected(record: &Value, options: &NormalizeOptions) {
    let error = import_records(&record.to_string(), "observer-jsonl", options).unwrap_err();
    assert!(error.is::<RedactionValidationError>(), "{error:#}");
}

#[test]
fn observer_envelope_context_preserves_actions_protocols_and_legacy_defaults() {
    let options = NormalizeOptions::default();
    for protocol in [Protocol::Systemone, Protocol::CloudflareSystemone] {
        for action in [false, true] {
            let mut original = observer(protocol, false);
            if action {
                original["event_kind"] = json!("application_action");
                original["actions"] = json!([{"kind":"synthetic_action"}]);
            }
            let output = restored(&original, &options);
            for field in [
                "event_kind",
                "protocol",
                "protocol_metadata",
                "model",
                "answers",
            ] {
                assert_eq!(output[field], original[field]);
            }
            let again = restored(&output, &options);
            for field in ["event_kind", "protocol", "protocol_metadata", "actions"] {
                assert_eq!(again[field], output[field]);
            }
        }
    }
    let mut legacy = observer(Protocol::Systemone, false);
    for field in ["event_kind", "protocol"] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    // A secret matching an absent optional field must not disable its ordinary
    // legacy default; no supplied interpretation is being discarded.
    legacy["request_extra"]["credentials"] = json!(["event_kind", "protocol"]);
    let output = restored(&legacy, &options);
    assert_eq!(output["event_kind"], "request");
    assert_eq!(output["protocol"], "systemone");
}

#[test]
fn observer_envelope_context_names_cannot_switch_actions_or_drop_protocol_envelopes() {
    let options = NormalizeOptions::default();
    let mut action = observer(Protocol::Systemone, false);
    action["event_kind"] = json!("application_action");
    action["actions"] = json!([{"kind":"synthetic_action"}]);
    action["request_extra"]["credentials"] = json!("event_kind");
    privacy_rejected(&action, &options);

    for answers in [false, true] {
        let mut record = observer(Protocol::CloudflareSystemone, answers);
        assert_eq!(record["protocol_metadata"]["success"], false);
        record["request_extra"]["credentials"] = json!("protocol");
        privacy_rejected(&record, &options);
    }
}

#[test]
fn observer_envelope_context_checks_values_and_final_generated_defaults() {
    for field in ["event_kind", "protocol"] {
        let options = NormalizeOptions {
            redact_keys: vec![field.into()],
            ..Default::default()
        };
        let mut record = observer(Protocol::Systemone, false);
        privacy_rejected(&record, &options);
        // The first pass has no supplied field to redact. The final pass still
        // must protect the generated request/System One interpretation.
        record.as_object_mut().unwrap().remove(field);
        privacy_rejected(&record, &options);
    }
}

#[test]
fn observer_envelope_context_redaction_cannot_repair_invalid_discriminators() {
    for field in ["event_kind", "protocol"] {
        for value in [
            json!(false),
            json!(1),
            json!([]),
            json!({}),
            json!("unsupported"),
        ] {
            let mut record = observer(Protocol::Systemone, false);
            record[field] = value;
            assert!(
                import_records(
                    &record.to_string(),
                    "observer-jsonl",
                    &NormalizeOptions::default()
                )
                .is_err()
            );
            record["request_extra"]["credentials"] = json!(field);
            privacy_rejected(&record, &NormalizeOptions::default());
        }
    }
    let mut action = observer(Protocol::Systemone, false);
    action["event_kind"] = Value::Null;
    action["request_extra"]["credentials"] = json!("event_kind");
    privacy_rejected(&action, &NormalizeOptions::default());
    // Explicit null protocol already means the legacy System One default.
    let mut legacy = observer(Protocol::Systemone, false);
    legacy["protocol"] = Value::Null;
    assert_eq!(
        restored(&legacy, &NormalizeOptions::default())["protocol"],
        "systemone"
    );
}
