use super::*;

fn empty_observer() -> Value {
    import_records(
        &json!({"id":"metadata-fixture","source":"metadata-fixture","provider":"custom",
            "timestamp":1_800_000_000_000_i64,"status":200,"capture_complete":true,
            "request":{"questions":{}},"response":{"answers":{}}})
        .to_string(),
        "systemone-capture",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0)
}

fn restored(record: &Value, options: &NormalizeOptions) -> Result<Vec<Value>> {
    import_records(&record.to_string(), "observer-jsonl", options)
}

#[test]
fn answerless_legacy_import_cannot_retain_an_invalid_generated_profile() {
    let mut legacy = empty_observer();
    legacy.as_object_mut().unwrap().remove("model_profile");
    let plain = restored(&legacy, &NormalizeOptions::default())
        .unwrap()
        .remove(0);
    assert!(plain["model_profile"].is_null());
    assert!(restored(&plain, &NormalizeOptions::default()).is_ok());

    let options = NormalizeOptions {
        redact_keys: vec!["model_profile".into()],
        ..Default::default()
    };
    let error = restored(&legacy, &options).unwrap_err();
    assert!(error.is::<RedactionValidationError>(), "{error:#}");
}

#[test]
fn unknown_event_time_cannot_retain_an_invalid_generated_import_time() {
    let mut legacy = empty_observer();
    legacy["timestamp"] = Value::Null;
    legacy.as_object_mut().unwrap().remove("imported_at");
    let plain = restored(&legacy, &NormalizeOptions::default())
        .unwrap()
        .remove(0);
    assert!(plain["timestamp"].is_null());
    assert!(plain["imported_at"].as_i64().is_some());
    assert_eq!(plain["timestamp_basis"], "import");
    let again = restored(&plain, &NormalizeOptions::default())
        .unwrap()
        .remove(0);
    assert_eq!(again["imported_at"], plain["imported_at"]);

    let options = NormalizeOptions {
        redact_keys: vec!["imported_at".into()],
        ..Default::default()
    };
    let error = restored(&legacy, &options).unwrap_err();
    assert!(format!("{error:#}").contains("imported_at must be valid epoch milliseconds"));
}
