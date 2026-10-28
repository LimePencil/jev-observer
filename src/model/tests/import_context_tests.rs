use super::*;

fn raw(event: &str, task: Value) -> Value {
    json!({"id":event,"source":"task-fixture","provider":"typesafe",
        "timestamp":1_800_000_000_000_i64,"status":200,"capture_complete":true,
        "task_version":task,
        "request":{"questions":{
            "first":{"type":"noul","instructions":"Apply the current rule"},
            "second":{"type":"noul","instructions":"Apply the current rule"}}},
        "response":{"answers":{
            "first":{"type":"noul","noul":0.8},
            "second":{"type":"noul","noul":0.4}}}})
}

fn imported(record: &Value, format: &str, options: &NormalizeOptions) -> Value {
    import_records(&record.to_string(), format, options)
        .unwrap()
        .remove(0)
}

fn observer(task: Value) -> Value {
    imported(
        &raw("task-event", task),
        "systemone-capture",
        &NormalizeOptions::default(),
    )
}

#[test]
fn observer_task_versions_reject_malformed_types_before_privacy_can_repair_them() {
    let original = observer(json!("rule-v1"));
    for task in [
        json!(false),
        json!(1),
        json!(2.5),
        json!([]),
        json!({"version":"rule-v1"}),
    ] {
        for options in [
            NormalizeOptions::default(),
            NormalizeOptions {
                redact_keys: vec!["task_version".into()],
                ..Default::default()
            },
        ] {
            for indexes in [&[0, 1][..], &[0][..], &[1][..]] {
                let mut record = original.clone();
                for &index in indexes {
                    record["answers"][index]["task_version"] = task.clone();
                }
                let error =
                    import_records(&record.to_string(), "observer-jsonl", &options).unwrap_err();
                assert!(format!("{error:#}").contains("task_version must be a string or null"));
            }
            let error = import_records(
                &raw("raw-event", task.clone()).to_string(),
                "systemone-capture",
                &options,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("task_version must be a string or null"));
        }
    }
}

#[test]
fn observer_task_versions_keep_distinct_valid_groups_and_legacy_unknown_context() {
    let options = NormalizeOptions::default();
    let mut groups = BTreeSet::new();
    for task in [
        Value::Null,
        json!(""),
        json!("rule-v1"),
        json!("rule-v2"),
        json!("규칙"),
    ] {
        let record = observer(task.clone());
        let restored = imported(&record, "observer-jsonl", &options);
        assert_eq!(restored["answers"], record["answers"]);
        assert_eq!(restored["answers"][0]["task_version"], task);
        assert!(groups.insert(restored["answers"][0]["group_id"].to_string()));
        assert_eq!(
            imported(&restored, "observer-jsonl", &options)["answers"],
            record["answers"]
        );
    }
    let mut legacy = observer(Value::Null);
    let expected = legacy["answers"].clone();
    for answer in legacy["answers"].as_array_mut().unwrap() {
        answer.as_object_mut().unwrap().remove("task_version");
    }
    assert_eq!(
        imported(&legacy, "observer-jsonl", &options)["answers"],
        expected
    );
}

#[test]
fn observer_task_versions_cannot_lose_supplied_context_when_field_names_are_redacted() {
    let options = NormalizeOptions::default();
    let mut raw_groups = BTreeSet::new();
    for task in ["rule-v1", "rule-v2"] {
        // Raw captures carry this context in generated envelope structure,
        // whose field names already survive credential redaction unchanged.
        let mut capture = raw(task, json!(task));
        capture["request"]["credentials"] = json!("task_version");
        let record = imported(&capture, "systemone-capture", &options);
        assert_eq!(record["answers"][0]["task_version"], task);
        assert!(raw_groups.insert(record["answers"][0]["group_id"].to_string()));
        assert_eq!(
            imported(&record, "observer-jsonl", &options)["answers"],
            record["answers"]
        );

        let mut record = observer(json!(task));
        record["request_extra"]["credentials"] = json!("task_version");
        let error = import_records(&record.to_string(), "observer-jsonl", &options).unwrap_err();
        assert!(error.is::<RedactionValidationError>(), "{error:#}");
    }
}

#[test]
fn observer_task_versions_validate_consistency_before_redacting_valid_context() {
    let options = NormalizeOptions {
        redact_keys: vec!["task_version".into()],
        ..Default::default()
    };
    for task in [Value::Null, json!("rule-v2")] {
        let mut mixed = observer(json!("rule-v1"));
        mixed["answers"][1]["task_version"] = task;
        let error = import_records(&mixed.to_string(), "observer-jsonl", &options).unwrap_err();
        assert!(format!("{error:#}").contains("Mixed task versions"));
    }
    let mut groups = BTreeSet::new();
    for task in ["rule-v1", "rule-v2"] {
        let record = imported(
            &raw(task, json!(task)),
            "systemone-capture",
            &NormalizeOptions::default(),
        );
        let restored = imported(&record, "observer-jsonl", &options);
        assert_eq!(restored["answers"][0]["task_version"], REDACTED);
        assert_eq!(restored["answers"][0]["definition_redacted"], true);
        assert!(groups.insert(restored["answers"][0]["group_id"].to_string()));
    }
}

fn media_capture(event: &str) -> Value {
    let mut capture = raw(event, Value::Null);
    capture["request"]["questions"]["first"]["instructions"] =
        json!("Compare against reference-image");
    capture["request"]["assets"] =
        json!({"reference-image":{"data_base64":format!("private-image-{event}")}});
    capture
}

fn media_observer(event: &str) -> Value {
    imported(
        &media_capture(event),
        "systemone-capture",
        &NormalizeOptions::default(),
    )
}

#[test]
fn observer_isolation_flags_keep_omitted_media_groups_separate_through_roundtrips() {
    let options = NormalizeOptions::default();
    let mut groups = BTreeSet::new();
    for event in ["image-one", "image-two"] {
        let record = media_observer(event);
        assert!(!record.to_string().contains("private-image-"));
        assert_eq!(record["answers"][0]["definition_redacted"], true);
        assert_eq!(record["answers"][1]["definition_redacted"], false);
        assert!(groups.insert(record["answers"][0]["group_id"].to_string()));
        let restored = imported(&record, "observer-jsonl", &options);
        for index in [0, 1] {
            for field in ["group_id", "definition_redacted", "definition", "valid"] {
                assert_eq!(
                    restored["answers"][index][field],
                    record["answers"][index][field]
                );
            }
        }
        assert_eq!(
            imported(&restored, "observer-jsonl", &options)["answers"],
            restored["answers"]
        );
    }
    let mut legacy = observer(Value::Null);
    let expected = legacy["answers"].clone();
    for answer in legacy["answers"].as_array_mut().unwrap() {
        answer
            .as_object_mut()
            .unwrap()
            .remove("definition_redacted");
    }
    assert_eq!(
        imported(&legacy, "observer-jsonl", &options)["answers"],
        expected
    );
}

#[test]
fn observer_isolation_flags_cannot_disappear_when_names_or_values_are_redacted() {
    let mut capture = media_capture("raw-isolation");
    capture["request"]["credentials"] = json!("definition_redacted");
    let preserved = imported(&capture, "systemone-capture", &NormalizeOptions::default());
    assert_eq!(preserved["answers"][0]["definition_redacted"], true);
    let restored = imported(&preserved, "observer-jsonl", &NormalizeOptions::default());
    assert_eq!(restored["answers"][0]["definition_redacted"], true);
    assert_eq!(
        restored["answers"][0]["group_id"],
        preserved["answers"][0]["group_id"]
    );

    let mut record = media_observer("redacted-image");
    record["request_extra"]["credentials"] = json!("definition_redacted");
    let error = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap_err();
    assert!(error.is::<RedactionValidationError>(), "{error:#}");

    let options = NormalizeOptions {
        redact_keys: vec!["definition_redacted".into()],
        ..Default::default()
    };
    let error = import_records(
        &media_observer("private-flag").to_string(),
        "observer-jsonl",
        &options,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("definition_redacted must be a boolean when supplied"));
}

#[test]
fn observer_isolation_flags_reject_malformed_supplied_types() {
    for flag in [
        Value::Null,
        json!(0),
        json!(1),
        json!("true"),
        json!([]),
        json!({}),
    ] {
        for options in [
            NormalizeOptions::default(),
            NormalizeOptions {
                redact_keys: vec!["definition_redacted".into()],
                ..Default::default()
            },
        ] {
            let mut record = media_observer("malformed-flag");
            record["answers"][0]["definition_redacted"] = flag.clone();
            let error =
                import_records(&record.to_string(), "observer-jsonl", &options).unwrap_err();
            assert!(
                format!("{error:#}")
                    .contains("definition_redacted must be a boolean when supplied")
            );
        }
    }
}

#[test]
fn observer_isolation_flags_validate_generated_context_after_final_privacy() {
    let mut legacy = observer(Value::Null);
    for answer in legacy["answers"].as_array_mut().unwrap() {
        answer
            .as_object_mut()
            .unwrap()
            .remove("definition_redacted");
    }
    let options = NormalizeOptions {
        redact_keys: vec!["definition_redacted".into()],
        ..Default::default()
    };
    let error = import_records(&legacy.to_string(), "observer-jsonl", &options).unwrap_err();
    assert!(format!("{error:#}").contains("definition_redacted must be a boolean when supplied"));
}
