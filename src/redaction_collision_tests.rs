use super::*;

fn capture(request: Value, response: Value) -> Capture {
    Capture {
        id: CaptureId::new_live(),
        timestamp: 1_800_000_000_000,
        source: "private-source".into(),
        status: 200,
        duration_ms: 12.5,
        capture_complete: true,
        request: serde_json::to_vec(&request).unwrap(),
        response: serde_json::to_vec(&response).unwrap(),
        ..Default::default()
    }
}

fn pair(first: &str, second: &str) -> Capture {
    // Response order intentionally differs: silently taking the last member
    // would attach the first answer's value to the second question's definition.
    capture(
        json!({"questions":{
            first:{"type":"noul","instructions":"private-copper-rule"},
            second:{"type":"noul","instructions":"private-silver-rule"}
        }}),
        json!({"answers":{
            second:{"type":"noul","noul":0.8},
            first:{"type":"noul","noul":0.2}
        }}),
    )
}

fn raw(item: &Capture) -> Value {
    json!({"id":"fixture-event", "source":"fixture", "provider":"typesafe",
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
    assert_eq!(record["timestamp"], item.timestamp);
    assert_eq!(record["status"], item.status);
    assert_eq!(record["duration_ms"], item.duration_ms);
    assert!(!record.to_string().contains("private-"));
    assert!(record.to_string().len() < 2048);
    record
}

#[test]
fn redacted_question_and_answer_key_collisions_cannot_cross_pair_values() {
    let options = NormalizeOptions::default();
    for (first, second, secrets) in [
        ("audit-X", "audit-Y", vec!["X", "Y"]),
        ("audit-X", "audit-[REDACTED]", vec!["X"]),
        (
            "audit-private-key-alpha",
            "audit-private-key-beta",
            vec!["private-key-alpha", "private-key-beta"],
        ),
    ] {
        for location in ["request", "response", "both"] {
            let mut item = pair(first, second);
            if location == "request" {
                item.response = br#"{"answers":{}}"#.to_vec();
            } else if location == "response" {
                item.request = br#"{"questions":{}}"#.to_vec();
            }
            let mut request: Value = serde_json::from_slice(&item.request).unwrap();
            request["credentials"] = json!(secrets);
            item.request = serde_json::to_vec(&request).unwrap();
            let first_record = incomplete(&item, &options);
            let second_record = incomplete(&item, &options);
            assert_ne!(first_record["id"], second_record["id"]);
            collision(
                import_records(&raw(&item).to_string(), "systemone-capture", &options).unwrap_err(),
            );
            let again =
                import_records(&first_record.to_string(), "observer-jsonl", &options).unwrap();
            for field in ["id", "answers", "capture_complete", "normalization_error"] {
                assert_eq!(again[0][field], first_record[field]);
            }
        }
    }
}

fn choice() -> Capture {
    capture(
        json!({"questions":{"route":{"type":"choice","instructions":"Select an option",
            "criteria":{"option-X":"First option","option-Y":"Second option"}}}}),
        json!({"answers":{"route":{"type":"choice","choice":"option-X","confidence":0.7,
            "probabilities":{"option-X":0.8,"option-Y":1.0}}}}),
    )
}

#[test]
fn redacted_criteria_or_probability_keys_cannot_turn_invalid_choice_valid() {
    let options = NormalizeOptions::default();
    let control = normalize(&choice(), &options);
    assert_eq!(control["answers"][0]["valid"], false);
    assert!(
        control["answers"][0]["error"]
            .as_str()
            .unwrap()
            .contains("sum to one")
    );
    for location in ["criteria", "probabilities", "both"] {
        let mut item = choice();
        let mut request: Value = serde_json::from_slice(&item.request).unwrap();
        let mut response: Value = serde_json::from_slice(&item.response).unwrap();
        request["credentials"] = json!(["X", "Y"]);
        if location == "criteria" {
            response["answers"]["route"]["probabilities"] = Value::Null;
        } else if location == "probabilities" {
            request["questions"]["route"]["criteria"] = Value::Null;
        }
        item.request = serde_json::to_vec(&request).unwrap();
        item.response = serde_json::to_vec(&response).unwrap();
        incomplete(&item, &options);
        collision(
            import_records(&raw(&item).to_string(), "systemone-capture", &options).unwrap_err(),
        );
    }
    let mut observer = control;
    observer["request_extra"]["credentials"] = json!(["X", "Y"]);
    collision(import_records(&observer.to_string(), "observer-jsonl", &options).unwrap_err());
    let mut receipt: Value =
        serde_json::from_str(include_str!("../fixtures/model/jevrouter-receipt.json")).unwrap();
    receipt["credentials"] = json!(["X", "Y"]);
    receipt["raw_jev"]["probabilities"] = json!({"option-X":0.8,"option-Y":1.0});
    collision(import_records(&receipt.to_string(), "jevrouter-receipt", &options).unwrap_err());
}

#[test]
fn redacted_distinct_keys_and_existing_markers_remain_complete() {
    let options = NormalizeOptions::default();
    for (first, second, credentials) in [
        ("left-X", "right-Y", json!(["X", "Y"])),
        ("left-[REDACTED]", "right-[REDACTED]", Value::Null),
    ] {
        let mut item = pair(first, second);
        let mut request: Value = serde_json::from_slice(&item.request).unwrap();
        request["credentials"] = credentials;
        item.request = serde_json::to_vec(&request).unwrap();
        let record = normalize(&item, &options);
        assert_eq!(record["id"], item.id.as_str());
        assert_eq!(record["capture_complete"], true);
        assert!(record["normalization_error"].is_null());
        let answers = record["answers"].as_array().unwrap();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0]["key"], "left-[REDACTED]");
        assert_eq!(answers[0]["value"], 0.2);
        assert_eq!(answers[1]["key"], "right-[REDACTED]");
        assert_eq!(answers[1]["value"], 0.8);
        assert!(answers.iter().all(|answer| answer["valid"] == true));
        let imported =
            import_records(&raw(&item).to_string(), "systemone-capture", &options).unwrap();
        assert_eq!(imported[0]["answers"].as_array().unwrap().len(), 2);
    }
}

#[test]
fn removed_headers_and_wholly_private_subtrees_do_not_create_key_collisions() {
    for container in ["headers", "credentials"] {
        let mut request = json!({"questions":{"q":{"type":"noul","instructions":"X"}}});
        request[container] = json!({"X":"X", "Y":"Y", "authorization":["X","Y"]});
        let item = capture(request, json!({"answers":{"q":{"type":"noul","noul":0.2}}}));
        let record = normalize(&item, &NormalizeOptions::default());
        assert_eq!(record["capture_complete"], true);
        assert!(record["normalization_error"].is_null());
        assert_eq!(record["answers"][0]["valid"], true);
        assert_eq!(record["answers"][0]["definition"]["instructions"], REDACTED);
        assert!(record["request_extra"].get("headers").is_none());
    }
}

#[test]
fn collision_inside_later_omitted_state_still_fails_closed() {
    for capture_state in [false, true] {
        let options = NormalizeOptions {
            capture_state,
            ..Default::default()
        };
        let item = capture(
            json!({"questions":{}, "credentials":["X","Y"], "state":{"X":1,"Y":2}}),
            json!({"answers":{}}),
        );
        incomplete(&item, &options);
        collision(
            import_records(&raw(&item).to_string(), "systemone-capture", &options).unwrap_err(),
        );
    }
}

#[test]
fn collision_failure_preserves_the_consumed_redaction_budget() {
    let options = NormalizeOptions::default();
    let value = json!({"credentials":["X","Y"], "extension":{"X":1,"Y":2}});
    let mut remaining = REDACTION_LIMITS.work;
    collision(privacy(&value, &options, &[], &mut remaining).unwrap_err());
    assert!(remaining < REDACTION_LIMITS.work);
    let charged = REDACTION_LIMITS.work - remaining;
    remaining = charged - 1;
    let error = privacy(&value, &options, &[], &mut remaining).unwrap_err();
    assert!(error.is::<ImportLimitError>());
}
