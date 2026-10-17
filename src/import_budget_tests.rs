use super::*;

fn raw_capture(count: usize, task: Option<String>) -> Value {
    let questions: Map<_, _> = (0..count)
        .map(|index| {
            (
                format!("q{index}"),
                json!({"type":"noul", "instructions":"Synthetic fixture"}),
            )
        })
        .collect();
    let answers: Map<_, _> = (0..count)
        .map(|index| (format!("q{index}"), json!({"type":"noul", "noul":0.5})))
        .collect();
    json!({"id":"budget-event", "source":"fixture", "provider":"typesafe",
        "timestamp":1800000000000_i64, "status":200, "capture_complete":true,
        "duration_ms":1, "task_version":task,
        "request":{"questions":questions}, "response":{"answers":answers, "usage":{}}})
}

fn limited(text: &str, format: &str, record: usize, batch: usize) -> Result<Vec<Value>> {
    import_records_with_limits(
        text,
        format,
        &NormalizeOptions::default(),
        ImportLimits { record, batch },
    )
}

fn limit_kind(error: anyhow::Error, expected: &str) {
    assert_eq!(
        error
            .downcast_ref::<ImportLimitError>()
            .expect("typed resource limit")
            .0,
        expected
    );
}

fn observer() -> Value {
    import_records(
        &raw_capture(0, None).to_string(),
        "systemone-capture",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0)
}

#[test]
fn final_output_depth_rejection_does_not_charge_the_batch() {
    let mut batch = ImportBudget {
        limits: IMPORT_LIMITS,
        work: 0,
        normalized: 0,
    };
    let mut budget = ImportRecordBudget {
        batch: &mut batch,
        work: 0,
    };
    let value = (0..crate::normalized_json::MAX_DEPTH).fold(json!(0), |v, _| json!([v]));
    budget.finish(&value).unwrap();
    let charged = budget.batch.normalized;
    let wrapped = json!({"derived_wrapper": value});
    let error = budget.finish(&wrapped).unwrap_err();
    assert!(error.to_string().contains("nesting limit"));
    assert!(!error.is::<ImportLimitError>());
    assert_eq!(budget.batch.normalized, charged);
    budget.finish(&json!({"after_rejection": true})).unwrap();
    assert!(budget.batch.normalized > charged);
}

#[test]
fn tiny_answer_fanout_is_rejected_by_work_preflight() {
    let mut record = raw_capture(0, None);
    record["request"]["questions"] = Value::Object(
        (0..128)
            .map(|index| (format!("q{index}"), Value::Null))
            .collect(),
    );
    let text = record.to_string();
    assert!(text.len() < 2048);
    limit_kind(
        limited(&text, "systemone-capture", 32 * 1024, 64 * 1024).unwrap_err(),
        "normalization work",
    );
}

#[test]
fn raw_task_replication_is_charged_after_privacy_expansion() {
    for redact in [false, true] {
        let mut record = raw_capture(
            8,
            Some(if redact {
                "x".repeat(512)
            } else {
                "v".repeat(4096)
            }),
        );
        if redact {
            record["request_headers"] = json!({"Authorization":"Bearer x"});
        }
        let text = record.to_string();
        assert!(text.len() < 6000);
        limit_kind(
            limited(&text, "systemone-capture", 32 * 1024, 64 * 1024).unwrap_err(),
            "normalization work",
        );
    }
}

#[test]
fn annotation_defaults_are_charged_only_when_inserted() {
    for key in ["actions", "labels"] {
        let mut record = observer();
        record["source_event_id"] = json!("event".repeat(820));
        record[key] = json!(
            (0..8)
                .map(|index| if key == "actions" {
                    json!({})
                } else {
                    json!({"key":format!("q{index}"), "label":"correct"})
                })
                .collect::<Vec<_>>()
        );
        limit_kind(
            limited(&record.to_string(), "observer-jsonl", 32 * 1024, 64 * 1024).unwrap_err(),
            "normalization work",
        );
        for annotation in record[key].as_array_mut().unwrap() {
            annotation["source"] = json!("original");
            annotation["source_event_id"] = json!("original-event");
        }
        let accepted =
            limited(&record.to_string(), "observer-jsonl", 32 * 1024, 64 * 1024).unwrap();
        assert_eq!(accepted[0][key], record[key]);
    }
    let mut malformed = observer();
    malformed.as_object_mut().unwrap().remove("actions");
    let error = limited(
        &malformed.to_string(),
        "observer-jsonl",
        32 * 1024,
        64 * 1024,
    )
    .unwrap_err();
    assert!(!error.is::<ImportLimitError>());
}

#[test]
fn long_metadata_and_near_boundary_exports_reimport_without_identity_changes() {
    let record = raw_capture(2, Some("版本".repeat(700)));
    let mut expected = limited(
        &record.to_string(),
        "systemone-capture",
        16 * 1024,
        32 * 1024,
    )
    .unwrap()
    .remove(0);
    assert!(
        expected["answers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|answer| answer["valid"] == true)
    );
    assert_eq!(
        expected["answers"][0]["task_version"],
        record["task_version"]
    );
    expected["import_format"] = json!("observer-jsonl");
    let roundtrip = limited(
        &expected.to_string(),
        "observer-jsonl",
        16 * 1024,
        32 * 1024,
    )
    .unwrap();
    assert_eq!(roundtrip, vec![expected.clone()]);
    assert_eq!(
        limited(
            &roundtrip[0].to_string(),
            "observer-jsonl",
            16 * 1024,
            32 * 1024
        )
        .unwrap(),
        roundtrip
    );
}

#[test]
fn cumulative_work_and_actual_json_have_independent_limits() {
    let mut record = raw_capture(2, None);
    record["source"] = json!("s".repeat(4000));
    let text = std::iter::repeat_n(record.to_string(), 4)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        limited(
            &record.to_string(),
            "systemone-capture",
            32 * 1024,
            48 * 1024
        )
        .is_ok()
    );
    limit_kind(
        limited(&text, "systemone-capture", 32 * 1024, 48 * 1024).unwrap_err(),
        "normalization work",
    );

    // Fixed derived fields can exceed the exact JSON cap even when replication
    // work fits. Keep this second guard independently covered.
    let record = raw_capture(1, None);
    let mut batch = ImportBudget {
        limits: IMPORT_LIMITS,
        work: 0,
        normalized: 0,
    };
    let mut budget = ImportRecordBudget {
        batch: &mut batch,
        work: 0,
    };
    budget.structure(&record, "systemone-capture").unwrap();
    let normalized = import_capture(&record, &NormalizeOptions::default(), &mut budget).unwrap();
    let work = budget.work;
    assert!(json_size(&normalized, IMPORT_LIMITS.record, "normalized output").unwrap() > work);
    limit_kind(
        limited(&record.to_string(), "systemone-capture", work, 16 * 1024).unwrap_err(),
        "normalized output",
    );
    let text = std::iter::repeat_n(record.to_string(), 8)
        .collect::<Vec<_>>()
        .join("\n");
    limit_kind(
        limited(&text, "systemone-capture", 16 * 1024, work * 8).unwrap_err(),
        "normalized output",
    );
}

#[test]
fn budget_arithmetic_never_wraps_or_partially_charges() {
    let mut batch = ImportBudget {
        limits: IMPORT_LIMITS,
        work: 0,
        normalized: 0,
    };
    let mut record = ImportRecordBudget {
        batch: &mut batch,
        work: 0,
    };
    record.charge(1).unwrap();
    limit_kind(
        record.repeated(usize::MAX, 2).unwrap_err(),
        "normalization work",
    );
    limit_kind(record.charge(usize::MAX).unwrap_err(), "normalization work");
    assert_eq!(record.work, 1);
    assert_eq!(record.batch.work, 1);
    assert_eq!(
        json_size(&json!("é\n"), 10, "normalized output").unwrap(),
        6
    );
    limit_kind(
        json_size(&json!("é\n"), 5, "normalized output").unwrap_err(),
        "normalized output",
    );
}

#[test]
fn receipt_answers_remain_private_provenance_without_discarded_normalization() {
    let mut answers: Map<_, _> = (0..128)
        .map(|index| (format!("q{index}"), json!({"type":"noul", "noul":0.5})))
        .collect();
    answers.insert(
        "secret".into(),
        json!({"authorization":"Bearer synthetic-secret", "echo":"synthetic-secret"}),
    );
    let record = json!({"decision_id":"receipt-event", "request_id":"request-event", "mode":"decision_only",
        "status":"selected", "decision":{"kind":"choice", "candidates":[], "echo":"synthetic-secret"},
        "provenance":{"jev_provider":"fixture"}, "execution":{}, "fallback":{},
        "raw_jev":{"answers":answers, "extension":{"echo":"synthetic-secret"}}});
    let records = limited(
        &record.to_string(),
        "jevrouter-receipt",
        32 * 1024,
        64 * 1024,
    )
    .unwrap();
    let out = &records[0];
    assert_eq!(out["answers"], json!([]));
    assert_eq!(out["event_kind"], "application_action");
    assert!(!out.to_string().contains("synthetic-secret"));
    assert_eq!(
        out["receipt"]["raw_jev"]["answers"]
            .as_object()
            .unwrap()
            .len(),
        129
    );
    assert_eq!(out["actions"][0]["raw_jev"], out["receipt"]["raw_jev"]);
    assert_eq!(out["receipt"]["decision"]["echo"], REDACTED);
    assert_eq!(out["response_extra"]["extension"]["echo"], REDACTED);
}

#[test]
fn ten_thousand_ordinary_small_records_fit_the_batch_budget() {
    let text = (0..MAX_IMPORT_RECORDS)
        .map(|index| {
            let mut record = raw_capture(0, None);
            record["id"] = json!(format!("small-{index}"));
            record.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.len() < 8 * 1024 * 1024);
    assert_eq!(
        import_records(&text, "systemone-capture", &NormalizeOptions::default())
            .unwrap()
            .len(),
        MAX_IMPORT_RECORDS
    );
}
