use super::*;

fn scrub_test_string(value: &str, secrets: &[&str]) -> String {
    let mut matcher = SecretMatcher::new(secrets.to_vec(), REDACTION_LIMITS).unwrap();
    scrub_string(value, &mut matcher).unwrap()
}

#[test]
fn provider_score_discrepancy_is_preserved_with_a_warning() {
    // Captured from OpenRouter's Jev response on 2026-10-02.
    let request = json!({"questions":{"q":{"type":"score","instructions":"Rate","criteria":["Calm","Frustrated","Very angry"]}}});
    let answer = json!({"type":"score","score":0.87,"confidence":0.79,"probabilities":{"0":0.14,"1":0.86,"2":0.0},"legend":{"0":"Calm","1":"Frustrated","2":"Very angry"}});
    let record = normalized(&capture(
        request.clone(),
        json!({"answers":{"q":answer.clone()}}),
    ));
    assert_eq!(record["answers"][0]["valid"], true);
    assert_eq!(record["answers"][0]["value"], 0.87);
    assert_eq!(record["answers"][0]["raw_answer"], answer);
    assert_eq!(
        record["answers"][0]["warnings"].as_array().unwrap().len(),
        1
    );
    let imported = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(
        imported[0]["answers"][0]["warnings"],
        record["answers"][0]["warnings"]
    );
    for invalid in [json!(-0.01), json!(3.0), json!("0.87")] {
        let mut bad = answer.clone();
        bad["score"] = invalid;
        assert_eq!(
            normalized(&capture(request.clone(), json!({"answers":{"q":bad}})))["answers"][0]["valid"],
            false
        );
    }
}

#[test]
fn reported_cost_takes_precedence_and_keeps_its_basis_across_import() {
    let options = NormalizeOptions {
        provider: Some("openrouter".into()),
        input_price_per_million: Some(100.0),
        output_price_per_million: Some(100.0),
        ..Default::default()
    };
    let mut c = noul();
    let mut response: Value = serde_json::from_slice(&c.response).unwrap();
    response["usage"] = json!({"input_tokens":408,"output_tokens":71,"cost":0.000017136});
    c.response = serde_json::to_vec(&response).unwrap();
    let record = normalize(&c, &options);
    assert_eq!(record["provider"], "openrouter");
    assert_eq!(record["cost_usd"], 0.000017136);
    assert_eq!(record["cost_basis"], "provider_reported");
    assert!(record["price"].is_null());
    let imported = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(imported[0]["cost_usd"], record["cost_usd"]);
    assert_eq!(imported[0]["cost_basis"], record["cost_basis"]);
    for bad in [json!(-1.0), json!("0.1"), json!(true), Value::Null] {
        response["usage"]["cost"] = bad;
        c.response = serde_json::to_vec(&response).unwrap();
        assert_eq!(normalize(&c, &options)["cost_basis"], "configured_estimate");
        assert!(normalize(&c, &NormalizeOptions::default())["cost_usd"].is_null());
    }
    response["usage"] = json!({"cost":0.0});
    c.response = serde_json::to_vec(&response).unwrap();
    let free = normalize(&c, &options);
    assert_eq!(free["cost_usd"], 0.0);
    assert_eq!(free["cost_basis"], "provider_reported");
    assert!(free["input_tokens"].is_null());
}

#[test]
fn unrecognized_cost_units_stay_extra_and_do_not_replace_configured_usd_estimates() {
    let mut c = noul();
    let mut response: Value = serde_json::from_slice(&c.response).unwrap();
    response["usage"] = json!({"input_tokens":100,"output_tokens":10,"cost":17.0});
    c.response = serde_json::to_vec(&response).unwrap();
    for provider in [None, Some("typesafe"), Some("laya"), Some("custom")] {
        let mut options = NormalizeOptions {
            provider: provider.map(str::to_owned),
            ..Default::default()
        };
        let unknown = normalize(&c, &options);
        assert!(unknown["cost_usd"].is_null());
        assert!(unknown["cost_basis"].is_null());
        assert_eq!(unknown["usage_extra"]["cost"], 17.0);
        options.input_price_per_million = Some(10.0);
        options.output_price_per_million = Some(20.0);
        let estimated = normalize(&c, &options);
        assert_eq!(estimated["cost_usd"], 0.0012);
        assert_eq!(estimated["cost_basis"], "configured_estimate");
        assert_eq!(estimated["usage_extra"]["cost"], 17.0);
    }
}

#[test]
fn laya_rounded_distributions_and_extended_levels_roundtrip() {
    let options = NormalizeOptions {
        provider: Some("laya".into()),
        ..Default::default()
    };
    let levels: Vec<String> = (0..16).map(|n| format!("Level {n}")).collect();
    let probabilities: Map<String, Value> = (0..16)
        .map(|n| (n.to_string(), json!(if n == 0 { 0.0624 } else { 0.0625 })))
        .collect();
    let legend: Map<String, Value> = levels
        .iter()
        .enumerate()
        .map(|(n, label)| (n.to_string(), json!(label)))
        .collect();
    let c = capture(
        json!({"model":"english","questions":{"q":{"type":"score","instructions":"Rate","criteria":levels},"tag":{"type":"choice","instructions":"Choose","criteria":["a","b"]}}}),
        json!({"model":"laya-rl-agent","answers":{"q":{"type":"score","score":7.5,"confidence":0.0,"probabilities":probabilities,"legend":legend},"tag":{"type":"choice","choice":"a","probabilities":{"a":0.8,"b":0.2},"confidence":0.3,"answer_confidence":0.8}},"usage":{"input_tokens":42,"output_tokens":0,"truncated":false}}),
    );
    let record = normalize(&c, &options);
    assert!(
        record["answers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|answer| answer["valid"] == true)
    );
    assert_eq!(record["provider"], "laya");
    assert_eq!(record["output_tokens"], 0);
    assert!(record["cost_usd"].is_null());
    assert_eq!(record["usage_extra"]["truncated"], false);
    let imported = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert!(
        imported[0]["answers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|answer| answer["valid"] == true)
    );
    assert_eq!(
        imported[0]["answers"][1]["raw_answer"]["answer_confidence"],
        0.8
    );
}

fn capture(request: Value, response: Value) -> Capture {
    Capture {
        id: "request-1".into(),
        timestamp: 1_800_000_000_000,
        source: "test".into(),
        status: 200,
        duration_ms: 12.0,
        capture_complete: true,
        request: serde_json::to_vec(&request).unwrap(),
        response: serde_json::to_vec(&response).unwrap(),
        ..Capture::default()
    }
}

fn noul() -> Capture {
    capture(
        json!({"model":"jev-latest","state":"example","questions":{"urgent":{"type":"noul","instructions":"Urgent?"}}}),
        json!({"model":"jev-1.13","answers":{"urgent":{"type":"noul","noul":0.8}},"usage":{"input_tokens":100,"output_tokens":10}}),
    )
}

fn normalized(c: &Capture) -> Value {
    normalize(c, &NormalizeOptions::default())
}

#[test]
fn imported_timestamps_share_the_dashboard_date_domain() {
    let capture = noul();
    let observer = normalized(&capture);
    let raw = json!({
        "id":"time-domain", "source":"time-domain", "provider":"typesafe",
        "timestamp":capture.timestamp, "status":200, "capture_complete":true,
        "request":serde_json::from_slice::<Value>(&capture.request).unwrap(),
        "response":serde_json::from_slice::<Value>(&capture.response).unwrap()
    });
    for (format, mut record, key) in [
        ("observer-jsonl", observer.clone(), "timestamp"),
        ("observer-jsonl", observer, "imported_at"),
        ("systemone-capture", raw, "timestamp"),
    ] {
        for timestamp in [-8_640_000_000_000_000_i64, -1, 0, 1, 8_640_000_000_000_000] {
            record[key] = json!(timestamp);
            let imported =
                import_records(&record.to_string(), format, &NormalizeOptions::default()).unwrap();
            assert_eq!(imported[0][key], timestamp, "{format} {key}");
        }
        for invalid in [
            json!(-8_640_000_000_000_001_i64),
            json!(8_640_000_000_000_001_i64),
            json!(i64::MIN),
            json!(i64::MAX),
            json!(u64::MAX),
            json!(1.5),
            json!(true),
            json!("1700000000000"),
        ] {
            record[key] = invalid;
            let error = import_records(&record.to_string(), format, &NormalizeOptions::default())
                .unwrap_err();
            assert!(format!("{error:#}").contains(key), "{format}: {error:#}");
            assert!(!error.is::<ImportLimitError>());
        }
    }
}

#[test]
fn mixed_answers_count_cost_once_and_score_uses_level_indices() {
    let c = capture(
        json!({"questions":{
            "route":{"type":"choice","instructions":"Team?","criteria":{"a":null,"b":"Other"}},
            "priority":{"type":"score","instructions":"Rate","criteria":["Low","Medium","High"]},
            "urgent":{"type":"noul","instructions":"Urgent?"}
        }}),
        json!({"answers":{
        "route":{"type":"choice","choice":"b","probabilities":{"a":0.2,"b":0.8},"confidence":0.7},
        "priority":{"type":"score","score":1.6,"probabilities":{"0":0.1,"1":0.2,"2":0.7},"confidence":0.6,"legend":{"0":"Low","1":"Medium","2":"High"}},
        "urgent":{"type":"noul","noul":0.9,"confidence":0.99}
    },"usage":{"input_tokens":1_000_000,"output_tokens":50}}),
    );
    let out = normalize(
        &c,
        &NormalizeOptions {
            input_price_per_million: Some(2.0),
            output_price_per_million: Some(0.0),
            ..Default::default()
        },
    );
    assert_eq!(out["answers"].as_array().unwrap().len(), 3);
    assert!(
        out["answers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["valid"] == true),
        "{out}"
    );
    assert_eq!(out["cost_usd"], 2.0);
    assert!(out["answers"][2]["confidence"].is_null());
    assert!(out["answers"][0].get("cost_usd").is_none());
}

#[test]
fn invalid_answers_are_preserved_but_excluded() {
    for bad in [
        json!({"type":"noul","noul":true}),
        json!({"type":"noul","noul":1.01}),
        json!({"type":"choice","noul":0.5}),
        Value::Null,
    ] {
        let mut c = noul();
        c.response = serde_json::to_vec(&json!({"answers":{"urgent":bad}})).unwrap();
        let out = normalized(&c);
        assert_eq!(out["answers"][0]["valid"], false);
        assert_eq!(out["answers"][0]["raw_answer"], bad);
    }
    let mut c = noul();
    c.status = 529;
    assert_eq!(normalized(&c)["answers"][0]["valid"], false);
    c.status = 200;
    c.capture_complete = false;
    assert_eq!(normalized(&c)["answers"][0]["valid"], false);
    c.capture_complete = true;
    c.transport_error = Some("connection reset".into());
    assert_eq!(normalized(&c)["answers"][0]["valid"], false);
}

#[test]
fn answers_cannot_supply_the_missing_question_type() {
    for (mut definition, raw) in [
        (
            json!({"type":"noul","instructions":"Urgent?"}),
            json!({"type":"noul","noul":0.8}),
        ),
        (
            json!({"type":"choice","instructions":"Pick","criteria":{"a":null,"b":null}}),
            json!({"type":"choice","choice":"a","probabilities":{"a":0.8,"b":0.2},"confidence":0.7}),
        ),
        (
            json!({"type":"score","instructions":"Rate","criteria":["Low","High"]}),
            json!({"type":"score","score":0.8,"probabilities":{"0":0.2,"1":0.8},"legend":{"0":"Low","1":"High"},"confidence":0.7}),
        ),
    ] {
        let mut c = capture(
            json!({"questions":{"q":definition}}),
            json!({"answers":{"q":raw}}),
        );
        assert_eq!(normalized(&c)["answers"][0]["valid"], true);
        definition.as_object_mut().unwrap().remove("type");
        c.request = serde_json::to_vec(&json!({"questions":{"q":definition}})).unwrap();
        let mut record = normalized(&c);
        assert_eq!(record["answers"][0]["valid"], false, "{record}");
        assert_eq!(record["answers"][0]["raw_answer"], raw);
        assert!(
            record["answers"][0]["error"]
                .as_str()
                .unwrap()
                .contains("Question type")
        );
        record["answers"][0]["valid"] = json!(true);
        let imported = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(imported[0]["answers"][0]["valid"], false);
    }
}

#[test]
fn distributions_and_score_consistency_are_checked() {
    let request = json!({"questions":{"q":{"type":"score","instructions":"Rate","criteria":["Low","Medium","High"]}}});
    let valid = json!({"type":"score","score":1.0,"probabilities":{"0":0.0,"1":1.0,"2":0.0},"legend":{"0":"Low","1":"Medium","2":"High"},"confidence":1.0});
    for (field, bad) in [
        ("score", json!(3.0)),
        ("confidence", json!(-0.1)),
        ("legend", json!({"1":"Low","2":"Medium","3":"High"})),
        ("probabilities", json!({"0":0.2,"1":0.2,"2":0.2})),
        ("probabilities", json!({"0":0.0,"1":1.0,"other":0.0})),
    ] {
        let mut raw = valid.clone();
        raw[field] = bad;
        assert_eq!(
            normalized(&capture(request.clone(), json!({"answers":{"q":raw}})))["answers"][0]["valid"],
            false
        );
    }
    let choice = capture(
        json!({"questions":{"q":{"type":"choice","instructions":"Pick","criteria":{"a":null,"b":null}}}}),
        json!({"answers":{"q":{"type":"choice","choice":"a","probabilities":{"a":0.1,"b":0.9},"confidence":0.8}}}),
    );
    assert_eq!(normalized(&choice)["answers"][0]["valid"], false);
}

#[test]
fn unknowns_are_not_zero_or_requested_model() {
    let mut c = noul();
    c.response = b"{\"answers\":{}}".to_vec();
    let out = normalized(&c);
    for key in [
        "input_tokens",
        "output_tokens",
        "cost_usd",
        "cost_basis",
        "model",
    ] {
        assert!(out[key].is_null());
    }
    assert!(out["answers"][0]["value"].is_null());
    assert_eq!(out["answers"][0]["valid"], false);
    c.request = b"{bad".to_vec();
    c.response = b"not-json-secret".to_vec();
    let out = normalized(&c);
    assert!(out["normalization_error"].is_string());
    assert!(!out.to_string().contains("not-json-secret"));
}

#[test]
fn strict_identity_ignores_examples_and_models_but_preserves_semantics() {
    let a = noul();
    let base = normalized(&a);
    let mut b = a.clone();
    let mut request: Value = serde_json::from_slice(&b.request).unwrap();
    request["state"] = json!("different example");
    request["model"] = json!("different-model");
    request["questions"]["neighbor"] = json!({"type":"noul","instructions":"Other?"});
    b.request = serde_json::to_vec(&request).unwrap();
    assert_eq!(
        base["answers"][0]["group_id"],
        normalized(&b)["answers"][0]["group_id"]
    );
    b.source = "another-source".into();
    assert_ne!(
        base["answers"][0]["group_id"],
        normalized(&b)["answers"][0]["group_id"]
    );
    b.source = a.source.clone();
    b.task_version = Some("rule-v2".into());
    assert_ne!(
        base["answers"][0]["group_id"],
        normalized(&b)["answers"][0]["group_id"]
    );
    b.task_version = None;
    request["questions"]["urgent"]["instructions"] = json!("Urgent by age 18?");
    b.request = serde_json::to_vec(&request).unwrap();
    assert_ne!(
        base["answers"][0]["definition_id"],
        normalized(&b)["answers"][0]["definition_id"]
    );
}

#[test]
fn canonical_definition_and_order_sensitive_presentation_are_distinct() {
    let mut a = noul();
    let mut b = a.clone();
    a.request=br#"{"questions":{"q":{"type":"choice","instructions":"Pick","criteria":{"a":"A","b":"B"}}}}"#.to_vec();
    b.request=br#"{"questions":{"q":{"criteria":{"b":"B","a":"A"},"instructions":"Pick","type":"choice"}}}"#.to_vec();
    let a = normalized(&a);
    let b = normalized(&b);
    assert_eq!(
        a["answers"][0]["definition_id"],
        b["answers"][0]["definition_id"]
    );
    assert_ne!(
        a["answers"][0]["presentation_id"],
        b["answers"][0]["presentation_id"]
    );
    assert_ne!(a["answers"][0]["group_id"], b["answers"][0]["group_id"]);
    assert_eq!(
        a["answers"][0]["candidate_id"],
        b["answers"][0]["candidate_id"]
    );
}

#[test]
fn changed_candidate_descriptions_and_score_order_change_identity() {
    let mut a = noul();
    a.request=serde_json::to_vec(&json!({"questions":{"q":{"type":"choice","instructions":"Pick","criteria":{"a0":"First person","a1":"Second person"}}}})).unwrap();
    let mut b = a.clone();
    let mut req: Value = serde_json::from_slice(&b.request).unwrap();
    req["questions"]["q"]["criteria"]["a0"] = json!("Different person");
    b.request = serde_json::to_vec(&req).unwrap();
    assert_ne!(
        normalized(&a)["answers"][0]["candidate_id"],
        normalized(&b)["answers"][0]["candidate_id"]
    );
    assert_ne!(
        fingerprint("def", &json!({"criteria":["Low","High"]})),
        fingerprint("def", &json!({"criteria":["High","Low"]}))
    );
}

#[test]
fn credentials_and_custom_keys_are_removed_everywhere_retained() {
    let secret = "forwarded-credential-abc";
    let mut c = capture(
        json!({"state":{"password":"nested-password","message":secret},"model":secret,"unknown":{"Authorization":format!("Bearer {secret}"),"echo":secret},"questions":{"q":{"type":"noul","instructions":format!("contains {secret}"),"email":"person@example.test","extra":"nested-password"}}}),
        json!({"answers":{"q":{"type":"noul","noul":0.9,"unknown":{"secret":secret,"echo":secret}}},"other":secret,"headers":{"x-private":secret}}),
    );
    c.id = secret.into();
    c.source = secret.into();
    c.task_version = Some(secret.into());
    c.transport_error = Some(secret.into());
    c.secret = Some(secret.into());
    let options = NormalizeOptions {
        capture_state: true,
        redact_keys: vec!["email".into()],
        ..Default::default()
    };
    let out = normalize(&c, &options);
    let text = out.to_string();
    for forbidden in [
        secret,
        "nested-password",
        "person@example.test",
        "x-private",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} leaked: {text}");
    }
    assert_eq!(out["state_retained"], true);
    assert!(out["answers"][0]["definition"]["extra"].is_string());
    assert_eq!(out["response_extra"]["other"], REDACTED);
}

#[test]
fn credential_redaction_deduplicates_and_preserves_overlap_priority() {
    let mut secrets = vec!["bcd".to_owned(), "abc".to_owned()];
    for _ in 0..2048 {
        secrets.extend(["bcd".to_owned(), "abc".to_owned()]);
    }
    secrets.extend(["long-secret".to_owned(), "long-secret".to_owned()]);
    let unique = ordered_unique_secrets(&secrets);
    assert_eq!(unique.len(), 3);
    assert_eq!(unique, ["long-secret", "bcd", "abc"]);
    assert_eq!(
        scrub_test_string("abcd long-secret", &unique),
        format!("a{REDACTED} {REDACTED}")
    );
    let reverse_secrets = ["abc".into(), "bcd".into()];
    let reverse = ordered_unique_secrets(&reverse_secrets);
    assert_eq!(scrub_test_string("abcd", &reverse), format!("{REDACTED}d"));
    let longest_secrets = ["abc".into(), "abcd".into()];
    let longest = ordered_unique_secrets(&longest_secrets);
    assert_eq!(scrub_test_string("abcd", &longest), REDACTED);
}

#[test]
fn credential_redaction_bounds_markers_and_preserves_utf8() {
    for (value, secrets, expected) in [
        ("EDE", vec!["E", "D", "E", "D"], REDACTED.repeat(3)),
        ("é秘密é", vec!["秘密", "é"], REDACTED.repeat(3)),
        ("abcabc", vec!["abc"], REDACTED.repeat(2)),
        ("", vec!["x"], String::new()),
        ("unchanged", vec!["missing", ""], "unchanged".into()),
    ] {
        let output = scrub_test_string(value, &secrets);
        assert_eq!(output.len(), expected.len());
        assert_eq!(output, expected);
        assert!(output.len() <= value.len() * REDACTED.len());
    }
}

#[test]
fn normalization_never_rescans_generated_credential_markers() {
    let mut c = capture(
        json!({
            "password": ["E", "D", "E", "D", "E", "D"],
            "message": "E",
            "questions": {"q": {"type": "noul", "instructions": "E"}}
        }),
        json!({"answers": {"q": {"type": "noul", "noul": 0.7}}, "E": "E"}),
    );
    c.secret = Some("E".into());
    c.local_token = Some("D".into());
    let output = normalized(&c);
    assert!(output.to_string().len() < 4096);
    assert_eq!(output["request_extra"]["message"], REDACTED);
    assert_eq!(output["response_extra"][REDACTED], REDACTED);
    assert_eq!(output["answers"][0]["definition"]["instructions"], REDACTED);
    assert_eq!(output["answers"][0]["definition_redacted"], true);
}

#[test]
fn structured_sensitive_values_cannot_leak_through_provider_echoes() {
    let options = NormalizeOptions {
        capture_state: true,
        redact_keys: vec!["private_contact".into()],
        ..Default::default()
    };
    let c = capture(
        json!({
            "headers": {"Authorization": ["Bearer   array-header-credential"]},
            "state": {"private_contact": {"addresses": ["private@example.test"]}, "client_token": "local-client-token-secret"},
            "questions": {"q": {"type": "noul", "instructions": "Flag this?"}}
        }),
        json!({
            "answers": {"q": {"type": "noul", "noul": 0.7}},
            "debug": "array-header-credential private@example.test local-client-token-secret"
        }),
    );
    let output = normalize(&c, &options);
    let retained = output.to_string();
    assert!(!retained.contains("array-header-credential"), "{retained}");
    assert!(!retained.contains("private@example.test"), "{retained}");
    assert!(
        !retained.contains("local-client-token-secret"),
        "{retained}"
    );
    assert_eq!(
        output["response_extra"]["debug"],
        format!("{REDACTED} {REDACTED} {REDACTED}")
    );

    let mut exported = normalized(&noul());
    exported["request_extra"] = json!({"private_key": ["imported-array-credential"]});
    exported["response_extra"] = json!({"echo": "imported-array-credential"});
    let imported = import_records(&exported.to_string(), "observer-jsonl", &options).unwrap();
    assert!(
        !imported[0]
            .to_string()
            .contains("imported-array-credential")
    );
}

#[test]
fn imported_workspace_access_headers_redact_echoes_before_grouping() {
    let token = format!("jo_access_{}", "a".repeat(64));
    let echo_key = format!("echo-{token}");
    let definition = json!({
        "type": "noul",
        "instructions": format!("Does {token} apply?")
    });
    let response = json!({"type": "noul", "noul": 0.7});
    let options = NormalizeOptions::default();
    for format in ["observer-jsonl", "systemone-capture"] {
        for (header, value) in [
            ("X-Observer-Access", json!(token)),
            ("x-observer-access", json!([token])),
        ] {
            let mut record = if format == "observer-jsonl" {
                let mut record = normalized(&noul());
                record["answers"][0]["definition"] = definition.clone();
                record["request_extra"] = json!({"echo": token});
                record["response_extra"] = json!({echo_key.clone(): token});
                record
            } else {
                json!({
                    "id": "first",
                    "source": "imported-sdk",
                    "provider": "typesafe",
                    "timestamp": 1_000,
                    "status": 200,
                    "capture_complete": true,
                    "duration_ms": 10.0,
                    "request": {"questions": {"q": definition}, "echo": token},
                    "response": {"answers": {"q": response}, echo_key.clone(): token}
                })
            };
            record["request_headers"] = json!({header: value});
            let mut groups = Vec::new();
            for id in ["first", "second"] {
                record["id"] = json!(id);
                let imported = import_records(&record.to_string(), format, &options).unwrap();
                let retained = &imported[0];
                assert!(
                    !retained.to_string().contains(&token),
                    "{format}: {retained}"
                );
                assert_eq!(retained["request_extra"]["echo"], REDACTED);
                assert_eq!(
                    retained["response_extra"][format!("echo-{REDACTED}")],
                    REDACTED
                );
                let answer = &retained["answers"][0];
                assert_eq!(
                    answer["definition"]["instructions"],
                    format!("Does {REDACTED} apply?")
                );
                assert_eq!(answer["definition_redacted"], true);
                assert!(answer["family_id"].is_null());
                groups.push(answer["group_id"].clone());
            }
            assert_ne!(
                groups[0], groups[1],
                "{format}: redacted definitions pooled"
            );
        }
    }
}

#[test]
fn untimed_observer_imports_get_a_valid_import_time_without_fabricated_event_time() {
    let mut record = normalized(&noul());
    record["timestamp"] = Value::Null;
    let before = chrono::Utc::now().timestamp_millis();
    let imported = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    let after = chrono::Utc::now().timestamp_millis();
    assert!(imported[0]["timestamp"].is_null());
    let imported_at = imported[0]["imported_at"]
        .as_i64()
        .expect("Storage needs an import timestamp");
    assert!((before..=after).contains(&imported_at));
    assert_eq!(imported[0]["timestamp_basis"], "import");
    let again = import_records(
        &imported[0].to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(again[0]["imported_at"], imported_at);

    record["imported_at"] = Value::Null;
    let both_null = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert!(both_null[0]["timestamp"].is_null());
    assert!(both_null[0]["imported_at"].as_i64().unwrap() >= before);
    assert_eq!(both_null[0]["timestamp_basis"], "import");

    record["imported_at"] = json!(before - 1_000);
    record["timestamp_basis"] = json!("provider");
    let supplied_time = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(supplied_time[0]["imported_at"], before - 1_000);
    assert_eq!(supplied_time[0]["timestamp_basis"], "import");

    record["timestamp"] = json!(before - 2_000);
    record["imported_at"] = Value::Null;
    let timed = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(timed[0]["timestamp"], before - 2_000);
    assert!(timed[0]["imported_at"].is_null());
    assert_eq!(timed[0]["timestamp_basis"], "provider");

    for invalid in [json!("yesterday"), json!(1.5), json!(u64::MAX)] {
        record["imported_at"] = invalid;
        assert!(
            import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default()
            )
            .is_err()
        );
    }
}

#[test]
fn usage_counts_must_fit_storage_before_they_can_establish_known_cost() {
    let options = NormalizeOptions {
        input_price_per_million: Some(1.0),
        output_price_per_million: Some(1.0),
        ..Default::default()
    };
    for field in ["input_tokens", "output_tokens"] {
        let mut c = noul();
        let mut response: Value = serde_json::from_slice(&c.response).unwrap();
        response["usage"] = json!({"input_tokens": 10, "output_tokens": 3});
        response["usage"][field] = json!(i64::MAX as u64 + 1);
        c.response = serde_json::to_vec(&response).unwrap();
        let normalized = normalize(&c, &options);
        assert!(normalized[field].is_null());
        assert!(normalized["cost_usd"].is_null());
        assert!(normalized["cost_basis"].is_null());

        let mut record = normalized.clone();
        record[field] = json!(i64::MAX as u64 + 1);
        assert!(import_records(&record.to_string(), "observer-jsonl", &options).is_err());
        record[field] = json!(i64::MAX);
        let imported = import_records(&record.to_string(), "observer-jsonl", &options).unwrap();
        assert_eq!(imported[0][field], i64::MAX);
    }
}

#[test]
fn raw_state_requires_opt_in_and_unknown_fields_survive_redacted() {
    let mut c = noul();
    let mut req: Value = serde_json::from_slice(&c.request).unwrap();
    req["new_option"] = json!({"n":7});
    c.request = serde_json::to_vec(&req).unwrap();
    let out = normalized(&c);
    assert_eq!(out["state_retained"], false);
    assert!(out["state"].is_null());
    assert_eq!(out["request_extra"]["new_option"]["n"], 7);
    assert_eq!(
        normalize(
            &c,
            &NormalizeOptions {
                capture_state: true,
                ..Default::default()
            }
        )["state"],
        "example"
    );
}

#[test]
fn structured_definition_state_is_retained_as_semantics_not_input() {
    let mut c = noul();
    let mut req: Value = serde_json::from_slice(&c.request).unwrap();
    req["state"] = json!({"private_input":"do not persist this snapshot"});
    req["questions"]["urgent"]["instructions"] =
        json!({"state":"Evaluate California residency", "question":"Eligible?"});
    c.request = serde_json::to_vec(&req).unwrap();
    let first = normalized(&c);
    assert!(first["state"].is_null());
    assert_eq!(
        first["answers"][0]["definition"]["instructions"]["state"],
        "Evaluate California residency"
    );
    assert_eq!(first["answers"][0]["definition_redacted"], false);
    req["questions"]["urgent"]["instructions"]["state"] = json!("Evaluate Oregon residency");
    c.request = serde_json::to_vec(&req).unwrap();
    assert_ne!(
        first["answers"][0]["group_id"],
        normalized(&c)["answers"][0]["group_id"]
    );
}

#[test]
fn redacted_definitions_are_isolated_and_never_family_pooled() {
    let options = NormalizeOptions {
        redact_keys: vec!["private_rule".into()],
        ..Default::default()
    };
    let mut c = capture(
        json!({"questions":{"c0":{"type":"noul","instructions":"Look only at the chunk with id \"c0\". Does that code match this description: retries","private_rule":"limit 3"}}}),
        json!({"answers":{"c0":{"type":"noul","noul":0.8}}}),
    );
    c.adapter = Some("jgrep-v1".into());
    let first = normalize(&c, &options);
    c.id = "second-call".into();
    let second = normalize(&c, &options);
    assert_eq!(first["answers"][0]["definition_redacted"], true);
    assert!(first["answers"][0]["family_id"].is_null());
    assert_ne!(
        first["answers"][0]["group_id"],
        second["answers"][0]["group_id"]
    );
    let imported = import_records(&first.to_string(), "observer-jsonl", &options).unwrap();
    assert_eq!(first["answers"], imported[0]["answers"]);
    // Header removal has no marker left, but must remain isolated on import.
    let mut req: Value = serde_json::from_slice(&c.request).unwrap();
    req["questions"]["c0"]
        .as_object_mut()
        .unwrap()
        .remove("private_rule");
    req["questions"]["c0"]["headers"] = json!({"x-rule":"hidden rule"});
    c.request = serde_json::to_vec(&req).unwrap();
    let header_redacted = normalized(&c);
    assert_eq!(header_redacted["answers"][0]["definition_redacted"], true);
    let imported = import_records(
        &header_redacted.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(header_redacted["answers"], imported[0]["answers"]);
}

#[test]
fn imported_removed_definition_fields_cannot_hide_distinct_question_rules() {
    let mut c = capture(
        json!({"questions":{"c0":{"type":"noul","instructions":"Look only at the chunk with id \"c0\". Does that code match this description: retries"}}}),
        json!({"answers":{"c0":{"type":"noul","noul":0.8}}}),
    );
    c.adapter = Some("jgrep-v1".into());
    let mut first = normalized(&c);
    assert!(first["answers"][0]["family_id"].is_string());
    let mut second = first.clone();
    second["id"] = json!("separate-imported-observation");
    for (record, rule, credential) in [
        (
            &mut first,
            "retry at most three times",
            "first-imported-provider-credential",
        ),
        (
            &mut second,
            "retry at most five times",
            "second-imported-provider-credential",
        ),
    ] {
        record["answers"][0]["definition"]["headers"] = json!({
            "x-rule":rule, "Authorization":format!("Bearer {credential}")
        });
        // Imported claims cannot override evidence that normalization
        // removes part of the original definition.
        record["answers"][0]["definition_redacted"] = json!(false);
    }
    let imported = import_records(
        &format!("{first}\n{second}"),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    for record in &imported {
        let answer = &record["answers"][0];
        assert_eq!(answer["definition_redacted"], true);
        assert!(answer["family_id"].is_null());
        assert!(answer["definition"].get("headers").is_none());
        assert!(!record.to_string().contains("imported-provider-credential"));
        let again = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(again[0]["answers"], record["answers"]);
    }
    assert_ne!(
        imported[0]["answers"][0]["group_id"],
        imported[1]["answers"][0]["group_id"]
    );
}

#[test]
#[ignore = "import scaling measurement; run explicitly with --ignored --nocapture"]
fn observer_import_large_answer_sets_benchmark() {
    for count in [500, 2_000, 5_000] {
        let questions: Map<String, Value> = (0..count)
            .map(|i| {
                (
                    format!("question-{i}"),
                    json!({"type":"noul","instructions":"Check this?"}),
                )
            })
            .collect();
        let answers: Map<String, Value> = questions
            .keys()
            .map(|key| (key.clone(), json!({"type":"noul","noul":0.7})))
            .collect();
        let capture = Capture {
            id: "benchmark".into(),
            source: "benchmark".into(),
            timestamp: 1_800_000_000_000,
            status: 200,
            capture_complete: true,
            request: serde_json::to_vec(&json!({"questions":questions})).unwrap(),
            response: serde_json::to_vec(&json!({"answers":answers})).unwrap(),
            ..Default::default()
        };
        let options = NormalizeOptions::default();
        let text = normalize(&capture, &options).to_string();
        let mut measurements = Vec::new();
        for _ in 0..3 {
            let started = std::time::Instant::now();
            let result = import_records(&text, "observer-jsonl", &options).unwrap();
            assert_eq!(result[0]["answers"].as_array().unwrap().len(), count);
            measurements.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        eprintln!(
            "IMPORT_BENCH {}",
            json!({"answers":count,"bytes":text.len(),"ms":measurements})
        );
    }
}

#[test]
fn observer_import_matches_privacy_evidence_by_key_when_answers_reorder() {
    let mut record = normalized(&capture(
        json!({"questions":{
            "":{"type":"noul","instructions":"Empty key?"},
            "missing-definition":{"type":"noul","instructions":"Unavailable?"},
            "한글":{"type":"noul","instructions":"Private rule?"},
            "ordinary":{"type":"noul","instructions":"Visible rule?"}
        }}),
        json!({"answers":{
            "":{"type":"noul","noul":0.1},
            "missing-definition":{"type":"noul","noul":0.2},
            "한글":{"type":"noul","noul":0.3},
            "ordinary":{"type":"noul","noul":0.4}
        }}),
    ));
    let answers = record["answers"].as_array_mut().unwrap();
    answers[0]["definition_redacted"] = json!(true);
    answers[1]["definition"] = Value::Null;
    answers[2]["definition_redacted"] = json!(true);
    answers.rotate_left(1);
    let imported = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0);
    let imported_answers = imported["answers"].as_array().unwrap();
    assert_eq!(imported_answers.len(), 4);
    // A definition-less answer is rebuilt after the question definitions,
    // so input/output positions are deliberately different.
    assert_eq!(record["answers"][0]["key"], "missing-definition");
    assert_eq!(imported_answers[3]["key"], "missing-definition");
    for answer in imported_answers {
        let key = answer["key"].as_str().unwrap();
        assert_eq!(answer["definition_redacted"], matches!(key, "" | "한글"));
        assert_eq!(answer["valid"], key != "missing-definition");
        let original = record["answers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|original| original["key"] == key)
            .unwrap();
        assert_eq!(answer["raw_answer"], original["raw_answer"]);
        if matches!(key, "" | "한글") {
            assert_ne!(answer["group_id"], original["group_id"]);
            assert!(answer["family_id"].is_null());
        }
    }
    let duplicate = record["answers"][0].clone();
    record["answers"].as_array_mut().unwrap().push(duplicate);
    let error = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("Duplicate answer key"));
}

#[test]
fn literal_redaction_marker_cannot_enable_recurring_pooling() {
    let mut c = noul();
    let mut req: Value = serde_json::from_slice(&c.request).unwrap();
    req["questions"]["urgent"]["instructions"] = json!("Check [REDACTED] rule");
    c.request = serde_json::to_vec(&req).unwrap();
    let a = normalized(&c);
    c.id = "another-call".into();
    assert_ne!(
        a["answers"][0]["group_id"],
        normalized(&c)["answers"][0]["group_id"]
    );
}

#[test]
fn incomplete_usage_and_prices_never_produce_a_cost() {
    let options = NormalizeOptions {
        input_price_per_million: Some(1.0),
        output_price_per_million: Some(0.0),
        ..Default::default()
    };
    let mut c = noul();
    for usage in [
        json!({"input_tokens":-1,"output_tokens":0}),
        json!({"input_tokens":1.5,"output_tokens":0}),
        json!({"input_tokens":100}),
        json!({}),
    ] {
        c.response = serde_json::to_vec(
            &json!({"usage":usage,"answers":{"urgent":{"type":"noul","noul":0.8}}}),
        )
        .unwrap();
        assert!(normalize(&c, &options)["cost_usd"].is_null());
    }
    let c = noul();
    assert!(
        normalize(
            &c,
            &NormalizeOptions {
                input_price_per_million: Some(1.0),
                ..Default::default()
            }
        )["cost_usd"]
            .is_null()
    );
}

#[test]
fn indexed_family_is_explicit_exact_and_preserves_changed_numbers() {
    let mut c = capture(
        json!({"questions":{
            "c0":{"type":"noul","instructions":"Look only at the chunk with id \"c0\". Does that code match this description: age over 18"},
            "c1":{"type":"noul","instructions":"Look only at the chunk with id \"c1\". Does that code match this description: age over 18"},
            "c2":{"type":"noul","instructions":"Look only at the chunk with id \"c2\". Does that code match this description: age over 21"}
        }}),
        json!({"answers":{}}),
    );
    assert!(normalized(&c)["answers"][0]["family_id"].is_null());
    c.adapter = Some("jgrep-v1".into());
    let out = normalized(&c);
    assert_eq!(
        out["answers"][0]["family_id"],
        out["answers"][1]["family_id"]
    );
    assert_ne!(out["answers"][0]["group_id"], out["answers"][1]["group_id"]);
    assert_ne!(
        out["answers"][0]["family_id"],
        out["answers"][2]["family_id"]
    );
    let mut req: Value = serde_json::from_slice(&c.request).unwrap();
    req["questions"]["c0"]["instructions"] = json!(
        "Look only at the chunk with id \"c1\". Does that code match this description: age over 18"
    );
    c.request = serde_json::to_vec(&req).unwrap();
    assert!(normalized(&c)["answers"][0]["family_id"].is_null());
}

#[test]
fn observer_roundtrip_recomputes_identity_and_keeps_event_provenance() {
    let original = normalized(&noul());
    let mut exported = original.clone();
    exported["answers"][0]["group_id"] = json!("forged");
    exported["answers"][0]["valid"] = json!(false);
    exported["answers"][0]["value"] = json!(0.0);
    exported["labels"] =
        json!([{"key":"urgent","label":"correct","source":"manual","reviewer":"local"}]);
    let imported = import_records(
        &exported.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(imported[0]["answers"], original["answers"]);
    assert_eq!(imported[0]["source_event_id"], original["id"]);
    assert_eq!(imported[0]["labels"][0]["source"], "manual");
    let again = import_records(
        &imported[0].to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(again[0]["source_event_id"], imported[0]["source_event_id"]);
    assert_eq!(again[0]["answers"], imported[0]["answers"]);
}

#[test]
fn observer_roundtrip_preserves_malformed_definitions_and_revalidates_answers() {
    let options = NormalizeOptions::default();
    let mut groups = BTreeSet::new();
    for definition in [
        json!(42),
        json!(-1.5),
        json!(true),
        json!(""),
        json!("한글"),
        json!([]),
        json!([null, 1, {"instructions": "nested"}]),
        Value::Null,
    ] {
        for raw in [json!({"type": "noul", "noul": 0.8}), Value::Null] {
            let original = normalized(&capture(
                json!({"questions": {"q": definition}}),
                json!({"answers": {"q": raw}}),
            ));
            let answer = &original["answers"][0];
            assert_eq!(answer["definition"], definition);
            assert_eq!(answer["raw_answer"], raw);
            assert_eq!(answer["valid"], false);
            if !raw.is_null() {
                assert!(groups.insert(answer["group_id"].as_str().unwrap().to_owned()));
            }
            let mut exported = original.clone();
            for field in ["group_id", "definition_id", "presentation_id", "family_id"] {
                exported["answers"][0][field] = json!("forged");
            }
            exported["answers"][0]["valid"] = json!(true);
            exported["answers"][0]["value"] = json!(123);
            let imported = import_records(&exported.to_string(), "observer-jsonl", &options)
                .unwrap_or_else(|error| panic!("{definition}: {error:#}"))
                .remove(0);
            assert_eq!(imported["answers"], original["answers"]);
            let again = import_records(&imported.to_string(), "observer-jsonl", &options)
                .unwrap()
                .remove(0);
            assert_eq!(again["answers"], original["answers"]);
            assert_eq!(again["source_event_id"], original["id"]);
        }
    }
    // Null is retained evidence; an absent required field is still invalid.
    for field in ["definition", "raw_answer"] {
        let mut missing = normalized(&noul());
        missing["answers"][0].as_object_mut().unwrap().remove(field);
        assert!(import_records(&missing.to_string(), "observer-jsonl", &options).is_err());
    }
}

#[test]
fn observer_roundtrip_keeps_redacted_scalar_and_array_definitions_isolated() {
    let secret = "malformed-definition-test-secret";
    for (key, definition) in [
        ("q", json!([secret, {"api_key": secret}])),
        (
            "authorization",
            json!({"type": "noul", "instructions": secret}),
        ),
    ] {
        let mut groups = Vec::new();
        for id in ["first", "second"] {
            let mut c = capture(
                json!({"questions": {key: definition}}),
                json!({"answers": {key: {"type": "noul", "noul": 0.8}}}),
            );
            c.id = id.into();
            c.secret = Some(secret.into());
            let original = normalized(&c);
            assert!(!original.to_string().contains(secret));
            let answer = &original["answers"][0];
            assert!(!answer["definition"].is_object());
            assert_eq!(answer["valid"], false);
            assert_eq!(answer["definition_redacted"], true);
            assert!(answer["family_id"].is_null());
            let imported = import_records(
                &original.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default(),
            )
            .unwrap()
            .remove(0);
            assert!(!imported.to_string().contains(secret));
            assert_eq!(imported["answers"], original["answers"]);
            groups.push(imported["answers"][0]["group_id"].clone());
        }
        assert_ne!(groups[0], groups[1]);
    }
}

#[test]
fn observer_roundtrip_preserves_empty_question_and_label_keys() {
    let mut original = normalized(&capture(
        json!({"questions":{"":{"type":"noul","instructions":"Urgent?"}}}),
        json!({"answers":{"":{"type":"noul","noul":0.8}}}),
    ));
    original["labels"] = json!([{"key":"","label":"correct","source":"manual"}]);
    assert_eq!(original["answers"][0]["valid"], true);
    let imported = import_records(
        &original.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(imported[0]["answers"], original["answers"]);
    assert_eq!(imported[0]["labels"][0]["key"], "");
    assert_eq!(imported[0]["labels"][0]["label"], "correct");
    for invalid in [Value::Null, json!(42), json!({})] {
        let mut record = original.clone();
        record["answers"][0]["key"] = invalid.clone();
        assert!(
            import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default()
            )
            .is_err()
        );
        let mut record = original.clone();
        record["labels"][0]["key"] = invalid;
        assert!(
            import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default()
            )
            .is_err()
        );
    }
}

fn receipt() -> Value {
    serde_json::from_str(include_str!(
        "../../../fixtures/model/jevrouter-receipt.json"
    ))
    .unwrap()
}

#[test]
fn receipt_keeps_policy_separate_without_invented_definitions_or_timings() {
    let imported = import_records(
        &serde_json::to_string_pretty(&receipt()).unwrap(),
        "jevrouter-receipt",
        &NormalizeOptions::default(),
    )
    .unwrap();
    let r = &imported[0];
    for key in ["timestamp", "status", "duration_ms", "cost_usd"] {
        assert!(r[key].is_null(), "{key}");
    }
    assert_eq!(r["actions"][0]["decision"]["selected"], "safe");
    assert_eq!(r["event_kind"], "application_action");
    assert!(r["answers"].as_array().unwrap().is_empty());
    assert_eq!(
        r["actions"][0]["raw_jev"]["answers"]["tool"]["choice"],
        "unsafe"
    );
    assert!(r["labels"].as_array().unwrap().is_empty());
    assert!(!r.to_string().contains("input secret"));
    let roundtrip = import_records(
        &r.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert_eq!(roundtrip[0]["answers"], r["answers"]);
    let mut no_model = receipt();
    no_model["raw_jev"] = Value::Null;
    assert!(
        import_records(
            &no_model.to_string(),
            "jevrouter-receipt",
            &NormalizeOptions::default()
        )
        .unwrap()[0]["answers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn imported_receipt_inputs_obey_new_capture_preference() {
    let retained = import_records(
        &receipt().to_string(),
        "jevrouter-receipt",
        &NormalizeOptions {
            capture_state: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(retained[0].to_string().contains("input secret"));
    let scrubbed = import_records(
        &retained[0].to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    assert!(!scrubbed[0].to_string().contains("input secret"));
}

#[test]
fn import_is_atomic_strict_and_does_not_deduplicate_equal_independent_calls() {
    let mut a = normalized(&noul());
    let mut b = a.clone();
    b["id"] = json!("independent-2");
    assert_eq!(
        import_records(
            &format!("{a}\n{b}"),
            "observer-jsonl",
            &NormalizeOptions::default()
        )
        .unwrap()
        .len(),
        2
    );
    assert!(
        import_records(
            &format!("{a}\n{{broken"),
            "observer-jsonl",
            &NormalizeOptions::default()
        )
        .is_err()
    );
    a["duration_ms"] = json!(-1);
    assert!(
        import_records(
            &a.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default()
        )
        .is_err()
    );
    assert!(import_records("{}", "made-up-format", &NormalizeOptions::default()).is_err());
    for invalid in ["null", "false", "1", "[]", r#""not a record""#] {
        assert!(import_records(invalid, "observer-jsonl", &NormalizeOptions::default()).is_err());
    }
}

#[test]
fn samples_are_synthetic_with_versions_errors_families_and_labels() {
    let samples = sample_records();
    assert_eq!(samples.len(), 720);
    assert!(samples.iter().all(
        |r| r["sample"] == true && (r["cost_usd"].is_null() || r["cost_basis"] == "synthetic")
    ));
    assert!(samples.iter().any(|r| r["status"] == 529));
    assert!(
        samples
            .iter()
            .any(|r| !r["labels"].as_array().unwrap().is_empty())
    );
    assert!(
        samples
            .iter()
            .flat_map(|r| r["answers"].as_array().unwrap())
            .any(|a| a["family_id"].is_string())
    );
    assert!(
        samples
            .iter()
            .filter(|r| r["status"] == 200)
            .flat_map(|r| r["answers"].as_array().unwrap())
            .all(|a| a["valid"] == true)
    );
    let versions: BTreeSet<_> = samples
        .iter()
        .flat_map(|r| r["answers"].as_array().unwrap())
        .filter(|a| a["key"] == "support_routing")
        .map(|a| a["definition_id"].as_str().unwrap())
        .collect();
    assert_eq!(versions.len(), 2);
}
