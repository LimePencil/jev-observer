use super::*;

fn fixture(protocol: Protocol, missing_definition: bool, missing_answer: bool) -> Value {
    let (definition, answer) = if protocol == Protocol::OpenaiDecisions {
        (
            json!({"type":"predicate","instructions":"Inspect"}),
            json!({"type":"predicate","probability":0.8}),
        )
    } else {
        (
            json!({"type":"noul","instructions":"Inspect"}),
            json!({"type":"noul","noul":0.8}),
        )
    };
    let (questions, answers) = if protocol == Protocol::OpenaiDecisions {
        (
            if missing_definition {
                json!([definition])
            } else {
                json!([definition, definition])
            },
            if missing_answer {
                json!([answer])
            } else {
                json!([answer, answer])
            },
        )
    } else {
        (
            if missing_definition {
                json!({"first":definition})
            } else {
                json!({"first":definition,"second":definition})
            },
            if missing_answer {
                json!({"first":answer})
            } else {
                json!({"first":answer,"second":answer})
            },
        )
    };
    json!({"id":"presence-fixture","source":"presence-fixture","provider":"custom",
        "protocol":protocol,"timestamp":1_793_159_874_000_i64,"status":200,"duration_ms":1.0,
        "capture_complete":true,"request":{"questions":questions},"response":{"answers":answers}})
}

fn imported(record: &Value, format: &str) -> Value {
    import_records(&record.to_string(), format, &NormalizeOptions::default())
        .unwrap()
        .remove(0)
}

#[test]
fn explicit_null_definitions_and_answers_preserve_order_and_neighbor_validity() {
    for protocol in [Protocol::Systemone, Protocol::OpenaiDecisions] {
        for (null_definition, null_answer) in [(true, false), (false, true), (true, true)] {
            let mut original = fixture(protocol, false, false);
            for (body, field, replace) in [
                ("request", "questions", null_definition),
                ("response", "answers", null_answer),
            ] {
                if replace {
                    if protocol == Protocol::OpenaiDecisions {
                        original[body][field][0] = Value::Null;
                    } else {
                        original[body][field]["first"] = Value::Null;
                    }
                }
            }
            let record = imported(&original, "systemone-capture");
            assert_eq!(record["answers"][0]["valid"], false);
            assert_eq!(record["answers"][0]["definition_present"], true);
            assert_eq!(record["answers"][0]["raw_answer_present"], true);
            assert_eq!(record["answers"][1]["valid"], true);
            let roundtrip = imported(&record, "observer-jsonl");
            assert_eq!(roundtrip["answers"], record["answers"]);
            assert_eq!(
                imported(&roundtrip, "observer-jsonl")["answers"],
                record["answers"]
            );
        }
    }
}

#[test]
fn missing_entries_and_provider_only_answers_keep_original_count_validation() {
    for protocol in [Protocol::Systemone, Protocol::OpenaiDecisions] {
        for (missing_definition, missing_answer) in [(true, false), (false, true)] {
            let record = imported(
                &fixture(protocol, missing_definition, missing_answer),
                "systemone-capture",
            );
            let second = &record["answers"][1];
            assert_eq!(second["definition_present"], !missing_definition);
            assert_eq!(second["raw_answer_present"], !missing_answer);
            assert_eq!(second["valid"], false);
            if protocol == Protocol::OpenaiDecisions {
                assert_eq!(record["answers"][0]["valid"], false);
                assert!(
                    second["error"]
                        .as_str()
                        .unwrap()
                        .contains("answer count differs")
                );
            }
            assert_eq!(
                imported(&record, "observer-jsonl")["answers"],
                record["answers"]
            );
        }
    }
}

#[test]
fn legacy_presence_defaults_keep_non_null_and_missing_evidence() {
    for protocol in [Protocol::Systemone, Protocol::OpenaiDecisions] {
        for (missing_definition, missing_answer) in [(false, false), (true, false), (false, true)] {
            let record = imported(
                &fixture(protocol, missing_definition, missing_answer),
                "systemone-capture",
            );
            let mut legacy = record.clone();
            for answer in legacy["answers"].as_array_mut().unwrap() {
                let answer = answer.as_object_mut().unwrap();
                answer.remove("definition_present");
                answer.remove("raw_answer_present");
            }
            assert_eq!(
                imported(&legacy, "observer-jsonl")["answers"],
                record["answers"]
            );
        }
    }
    let mut original = fixture(Protocol::Systemone, false, true);
    original["request"]["questions"]["second"] = Value::Null;
    let record = imported(&original, "systemone-capture");
    let mut legacy = record.clone();
    let second = legacy["answers"][1].as_object_mut().unwrap();
    second.remove("definition_present");
    second.remove("raw_answer_present");
    assert_eq!(
        imported(&legacy, "observer-jsonl")["answers"],
        record["answers"]
    );
}

#[test]
fn supplied_presence_cannot_hide_original_evidence_or_accept_malformed_flags() {
    let record = imported(
        &fixture(Protocol::OpenaiDecisions, false, false),
        "systemone-capture",
    );
    for field in ["definition_present", "raw_answer_present"] {
        for value in [Value::Null, json!("true"), json!(1), json!(false)] {
            let mut changed = record.clone();
            changed["answers"][0][field] = value;
            assert!(
                import_records(
                    &changed.to_string(),
                    "observer-jsonl",
                    &NormalizeOptions::default()
                )
                .is_err()
            );
        }
        assert!(
            import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions {
                    redact_keys: vec![field.into()],
                    ..Default::default()
                }
            )
            .is_err()
        );
        // Scrubbing a presence field's name must not silently select legacy
        // reconstruction and turn an explicit null into a missing entry.
        let mut changed = record.clone();
        changed["request_extra"] = json!({"credentials":field});
        let error = import_records(
            &changed.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap_err();
        assert!(error.is::<RedactionValidationError>());
    }
    let mut changed = record;
    changed["answers"][0]["definition"] = Value::Null;
    changed["answers"][0]["raw_answer"] = Value::Null;
    changed["answers"][0]["definition_present"] = json!(false);
    changed["answers"][0]["raw_answer_present"] = json!(false);
    assert!(
        import_records(
            &changed.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default()
        )
        .is_err()
    );
}
