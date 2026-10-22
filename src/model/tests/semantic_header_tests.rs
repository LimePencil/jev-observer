use super::*;

const HEADER_NAMES: [&str; 4] = ["headers", "request_headers", "Response-Headers", "HeAdErS"];

fn capture(request: Value, response: Value) -> Capture {
    Capture {
        id: CaptureId::new_live(),
        timestamp: 1_800_000_000_000,
        source: "fixture".into(),
        status: 200,
        capture_complete: true,
        request: serde_json::to_vec(&request).unwrap(),
        response: serde_json::to_vec(&response).unwrap(),
        ..Default::default()
    }
}

fn records(request: Value, response: Value, options: &NormalizeOptions) -> Vec<Value> {
    let item = capture(request.clone(), response.clone());
    let raw = json!({"id":"fixture-event", "source":"fixture", "provider":options.provider.as_deref().unwrap_or("typesafe"),
        "timestamp":item.timestamp, "status":200, "capture_complete":true,
        "request":request, "response":response});
    // Build an Observer record from original evidence, not from an already
    // scrubbed result: its first privacy pass must preserve the same semantics.
    let mut observer = normalize(
        &capture(json!({"questions":{}}), json!({"answers":{}})),
        options,
    );
    let keys: BTreeSet<&str> = request["questions"]
        .as_object()
        .unwrap()
        .keys()
        .chain(response["answers"].as_object().unwrap().keys())
        .map(String::as_str)
        .collect();
    observer["answers"] = Value::Array(keys.into_iter().map(|key| json!({
        "key":key, "definition":request["questions"][key], "raw_answer":response["answers"][key]
    })).collect());
    observer["request_extra"] = request.clone();
    for field in ["questions", "state"] {
        observer["request_extra"]
            .as_object_mut()
            .unwrap()
            .remove(field);
    }
    observer["response_extra"] = response.clone();
    observer["response_extra"]
        .as_object_mut()
        .unwrap()
        .remove("answers");
    observer["state"] = request["state"].clone();
    observer["state_retained"] = json!(options.capture_state);
    vec![
        normalize(&item, options),
        import_records(&raw.to_string(), "systemone-capture", options)
            .unwrap()
            .remove(0),
        import_records(&observer.to_string(), "observer-jsonl", options)
            .unwrap()
            .remove(0),
    ]
}

fn answer<'a>(record: &'a Value, key: &str) -> &'a Value {
    assert_eq!(record["capture_complete"], true);
    assert!(record["normalization_error"].is_null());
    record["answers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["key"] == key)
        .unwrap()
}

#[test]
fn header_named_questions_and_answers_keep_original_pairing() {
    let options = NormalizeOptions::default();
    for name in HEADER_NAMES {
        for neighbor in [false, true] {
            let definition = json!({"type":"noul","instructions":"Copper rule"});
            let raw_answer = json!({"type":"noul","noul":0.2});
            let mut request = json!({"questions":{name:definition}});
            let mut response = json!({"answers":{name:raw_answer}});
            if neighbor {
                request["questions"]["neighbor"] =
                    json!({"type":"noul","instructions":"Silver rule"});
                response["answers"]["neighbor"] = json!({"type":"noul","noul":0.8});
            }
            for record in records(request, response, &options) {
                assert_eq!(
                    record["answers"].as_array().unwrap().len(),
                    if neighbor { 2 } else { 1 }
                );
                let retained = answer(&record, name);
                assert_eq!(retained["definition"], definition);
                assert_eq!(retained["raw_answer"], raw_answer);
                assert_eq!(retained["value"], 0.2);
                assert_eq!(retained["valid"], true);
                assert_eq!(retained["definition_redacted"], false);
                if neighbor {
                    assert_eq!(answer(&record, "neighbor")["value"], 0.8);
                }
            }
        }
        for missing_question in [false, true] {
            let request = if missing_question {
                json!({"questions":{}})
            } else {
                json!({"questions":{name:{"type":"noul","instructions":"Rule"}}})
            };
            let response = if missing_question {
                json!({"answers":{name:{"type":"noul","noul":0.2}}})
            } else {
                json!({"answers":{}})
            };
            for record in records(request, response, &options) {
                assert_eq!(record["answers"].as_array().unwrap().len(), 1);
                assert_eq!(answer(&record, name)["valid"], false);
            }
        }
    }
}

#[test]
fn header_named_choice_options_preserve_selected_and_unselected_probabilities() {
    for name in HEADER_NAMES {
        for select_header in [false, true] {
            for list in [false, true] {
                let options = NormalizeOptions {
                    provider: Some(if list { "laya" } else { "typesafe" }.into()),
                    ..Default::default()
                };
                let criteria = if list {
                    json!([name, "neighbor"])
                } else {
                    json!({name:"Header option","neighbor":"Neighbor option"})
                };
                let probabilities = if select_header {
                    json!({name:0.8,"neighbor":0.2})
                } else {
                    json!({name:0.2,"neighbor":0.8})
                };
                let selected = if select_header { name } else { "neighbor" };
                let definition = json!({"type":"choice","instructions":"Pick","criteria":criteria});
                let raw_answer = json!({"type":"choice","choice":selected,"confidence":0.7,"probabilities":probabilities});
                for record in records(
                    json!({"questions":{"q":definition}}),
                    json!({"answers":{"q":raw_answer}}),
                    &options,
                ) {
                    let retained = answer(&record, "q");
                    assert_eq!(retained["valid"], true, "{name}/{list}: {retained}");
                    assert_eq!(retained["definition"], definition);
                    assert_eq!(retained["raw_answer"], raw_answer);
                    assert_eq!(retained["probabilities"], probabilities);
                    assert_eq!(retained["value"], selected);
                }
            }
        }
    }
}

#[test]
fn invalid_header_named_validation_entries_cannot_disappear_and_become_valid() {
    let options = NormalizeOptions::default();
    for name in HEADER_NAMES {
        let score = json!({"type":"score","instructions":"Rate","criteria":["Low","High"]});
        let score_answer = json!({"type":"score","score":0.5,"confidence":0.7,"probabilities":{"0":0.5,"1":0.5},"legend":{"0":"Low","1":"High"}});
        let mut extra_probability = score_answer.clone();
        extra_probability["probabilities"][name] = json!(0.2);
        let mut extra_legend = score_answer;
        extra_legend["legend"][name] = json!("Extra");
        let cases = [
            (
                json!({"type":"noul","instructions":"Rule","criteria":{name:"Invalid criterion"}}),
                json!({"type":"noul","noul":0.2}),
            ),
            (
                json!({"type":"choice","instructions":"Pick","criteria":{name:null,"neighbor":null}}),
                json!({"type":"choice","choice":"neighbor","confidence":0.7,"probabilities":{name:0.8,"neighbor":1.0}}),
            ),
            (score.clone(), extra_probability),
            (score, extra_legend),
        ];
        for (definition, raw_answer) in cases {
            for record in records(
                json!({"questions":{"q":definition}}),
                json!({"answers":{"q":raw_answer}}),
                &options,
            ) {
                let retained = answer(&record, "q");
                assert_eq!(retained["definition"], definition);
                assert_eq!(retained["raw_answer"], raw_answer);
                assert_eq!(retained["valid"], false, "{name}: {retained}");
                assert!(retained["error"].is_string());
            }
        }
    }
}

#[test]
fn semantic_map_values_still_scrub_secrets_and_remove_nested_header_containers() {
    for capture_state in [false, true] {
        let options = NormalizeOptions {
            capture_state,
            ..Default::default()
        };
        // Only the direct question/option names are exempt. Headers beneath
        // their values, even in protocol-looking extensions, remain private.
        let nested = json!({"headers":{"Authorization":"Bearer synthetic-private-value"},
            "request_headers":{"x-trace":"discarded-header-value"},
            "Response-Headers":{"x-trace":"discarded-response-value"},
            "password":"synthetic-password-value", "echo":"synthetic-private-value"});
        let request = json!({
            "questions":{"headers":{"type":"choice","instructions":"Pick synthetic-private-value",
                "criteria":{"headers":{"description":"Option","extension":nested}},"extension":nested}},
            "headers":{"Authorization":"Bearer synthetic-private-value"},
            "extension":{"questions":{"headers":nested}},"state":nested
        });
        let response = json!({"answers":{"headers":{"type":"choice","choice":"headers","confidence":0.7,
            "probabilities":{"headers":1.0},"extension":nested}},"extension":nested});
        for record in records(request, response, &options) {
            let retained = answer(&record, "headers");
            assert_eq!(retained["valid"], true);
            assert_eq!(retained["probabilities"], json!({"headers":1.0}));
            assert_eq!(retained["definition"]["instructions"], "Pick [REDACTED]");
            assert_eq!(retained["definition_redacted"], true);
            let sanitized = json!({"password":REDACTED,"echo":REDACTED});
            assert_eq!(retained["definition"]["extension"], sanitized);
            assert_eq!(
                retained["definition"]["criteria"]["headers"]["extension"],
                sanitized
            );
            assert_eq!(retained["raw_answer"]["extension"], sanitized);
            assert_eq!(record["response_extra"]["extension"], sanitized);
            assert!(record["request_extra"].get("headers").is_none());
            assert_eq!(record["request_extra"]["extension"]["questions"], json!({}));
            assert_eq!(record["state_retained"], capture_state);
            assert_eq!(
                record["state"],
                if capture_state {
                    sanitized
                } else {
                    Value::Null
                }
            );
            for private in [
                "synthetic-private-value",
                "synthetic-password-value",
                "discarded-header-value",
                "discarded-response-value",
            ] {
                assert!(!record.to_string().contains(private));
            }
        }
    }
}

#[test]
fn receipt_provenance_does_not_receive_semantic_header_exemptions() {
    let mut receipt: Value = serde_json::from_str(include_str!(
        "../../../fixtures/model/jevrouter-receipt.json"
    ))
    .unwrap();
    receipt["raw_jev"]["questions"] =
        json!({"headers":{"type":"noul","instructions":"Not inference"}});
    receipt["raw_jev"]["criteria"] = json!({"headers":"Not an option"});
    receipt["raw_jev"]["headers"] = json!({"Authorization":"Bearer private-receipt-value"});
    receipt["raw_jev"]["echo"] = json!("private-receipt-value");
    let record = import_records(
        &receipt.to_string(),
        "jevrouter-receipt",
        &Default::default(),
    )
    .unwrap()
    .remove(0);
    assert_eq!(record["answers"], json!([]));
    assert_eq!(record["receipt"]["raw_jev"]["questions"], json!({}));
    assert_eq!(record["receipt"]["raw_jev"]["criteria"], json!({}));
    assert!(record["receipt"]["raw_jev"].get("headers").is_none());
    assert_eq!(record["receipt"]["raw_jev"]["echo"], REDACTED);
}
