use super::*;

fn envelope(provider: &str) -> Value {
    json!({"id":"call-1","timestamp":1790985600000_i64,"source":"model-fixture","provider":provider,"status":200,"duration_ms":null,"capture_complete":true,"sample":true,
        "request":{"model":"requested-alias","state":"private-input","images":["private-image"],"questions":{
            "route":{"type":"choice","instructions":"Choose","criteria":{"a":null,"b":"Other"}},
            "rating":{"type":"score","instructions":"Rate","criteria":["Low","High"]},
            "check":{"type":"noul","instructions":"Check"}}},
        "response":{"model":"reported-revision","answers":{
            "route":{"type":"choice","choice":"b","probabilities":{"a":0.2,"b":0.8},"confidence":0.6},
            "rating":{"type":"score","score":0.8,"probabilities":{"0":0.2,"1":0.8},"legend":{"0":"Low","1":"High"},"confidence":0.6},
            "check":{"type":"noul","noul":0.8}},"usage":{"input_tokens":12,"output_tokens":0,"cost":123}}})
}

fn imported(value: &Value) -> Value {
    import_records(
        &value.to_string(),
        "systemone-capture",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0)
}

#[test]
fn all_catalog_integrations_capture_typed_answers_and_roundtrip_without_invented_metadata() {
    for model in catalog::CATALOG["models"].as_array().unwrap() {
        let provider = model["id"].as_str().unwrap();
        let mut original = envelope(provider);
        if provider == "eikos" {
            original["response"]["answers"]["rating"]["score"] = json!(1);
            original["response"]["answers"]["rating"]["expected"] = json!(0.8);
        }
        let out = imported(&original);
        assert!(
            out["answers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|answer| answer["valid"] == true),
            "{provider}: {out}"
        );
        assert_eq!(out["provider"], provider);
        assert_eq!(out["model"], "reported-revision");
        assert_eq!(out["requested_model"], "requested-alias");
        assert!(out["duration_ms"].is_null());
        assert!(out["sample"] == true);
        assert!(!out.to_string().contains("private-input"));
        assert!(!out.to_string().contains("private-image"));
        if provider != "openrouter" {
            assert!(out["cost_usd"].is_null(), "{provider}");
        }
        let roundtrip = import_records(
            &out.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap()
        .remove(0);
        for field in [
            "answers",
            "id",
            "source_event_id",
            "model",
            "requested_model",
            "provider",
            "sample",
            "cost_usd",
        ] {
            assert_eq!(out[field], roundtrip[field], "{provider}: {field}");
        }
    }
}

#[test]
fn documented_probability_rounding_is_provider_scoped_and_never_renormalized() {
    for (provider, p) in [
        ("kev", 0.3333),
        ("decider", 0.3333),
        ("laya", 0.3333),
        ("milliseconds", 0.333),
    ] {
        let mut event = envelope(provider);
        event["request"]["questions"]["route"]["criteria"] = json!({"a":null,"b":null,"c":null});
        event["response"]["answers"]["route"]["choice"] = json!("a");
        let dist = json!({"a":p,"b":p,"c":p});
        event["response"]["answers"]["route"]["probabilities"] = dist.clone();
        let out = imported(&event);
        assert_eq!(out["answers"][0]["valid"], true, "{provider}: {out}");
        assert_eq!(out["answers"][0]["probabilities"], dist);
        event["response"]["answers"]["route"]["probabilities"]["c"] = json!(0.32);
        assert_eq!(imported(&event)["answers"][0]["valid"], false, "{provider}");
    }
    let mut strict = envelope("custom");
    strict["response"]["answers"]["route"]["probabilities"] = json!({"a":0.333,"b":0.666});
    assert_eq!(imported(&strict)["answers"][0]["valid"], false);
}

#[test]
fn extended_rubrics_structured_legends_and_missing_confidence_keep_original_evidence() {
    for (provider, count) in [
        ("kev", 255),
        ("nimble", 26),
        ("laya", 32),
        ("jeff", 1),
        ("openjev", 1),
    ] {
        let mut event = envelope(provider);
        event["request"]["questions"]["rating"]["criteria"] =
            json!((0..count).map(|n| format!("Level {n}")).collect::<Vec<_>>());
        let probabilities: Map<String, Value> = (0..count)
            .map(|n| (n.to_string(), json!(if n == 0 { 1.0 } else { 0.0 })))
            .collect();
        let legend: Map<String, Value> = (0..count)
            .map(|n| (n.to_string(), json!(format!("Level {n}"))))
            .collect();
        event["response"]["answers"]["rating"] = json!({"type":"score","score":0.0,"confidence":1.0,"probabilities":probabilities,"legend":legend});
        assert_eq!(imported(&event)["answers"][1]["valid"], true, "{provider}");
        event["provider"] = json!("typesafe");
        assert_eq!(
            imported(&event)["answers"][1]["valid"],
            false,
            "Strict contract must stay strict"
        );
    }
    let mut event = envelope("openjev-sglang");
    event["request"]["questions"]["rating"]
        .as_object_mut()
        .unwrap()
        .remove("instructions");
    event["request"]["questions"]["rating"]["criteria"] = json!([{"level":"Low"},["High"]]);
    event["response"]["answers"]["rating"]["legend"] = json!({"0":{"level":"Low"},"1":["High"]});
    assert_eq!(imported(&event)["answers"][1]["valid"], true);
    for provider in ["nanojev", "agentjev", "semif", "jevk5"] {
        let mut event = envelope(provider);
        event["response"]["answers"]["rating"]
            .as_object_mut()
            .unwrap()
            .remove("legend");
        if provider != "jevk5" {
            event["response"]["answers"]["rating"]
                .as_object_mut()
                .unwrap()
                .remove("confidence");
        }
        let out = imported(&event);
        assert_eq!(out["answers"][1]["valid"], true, "{provider}: {out}");
        assert!(!out["answers"][1]["warnings"].as_array().unwrap().is_empty());
        if provider != "jevk5" {
            assert!(out["answers"][1]["confidence"].is_null());
        }
        event["response"]["answers"]["rating"]["confidence"] = json!(2);
        assert_eq!(imported(&event)["answers"][1]["valid"], false);
    }
}

#[test]
fn raw_capture_import_recomputes_validity_isolates_redaction_and_has_explicit_identity() {
    let mut event = envelope("nanojev");
    event["request"]["questions"]["route"]["private_rule"] = json!("secret-rule-a");
    let options = NormalizeOptions {
        redact_keys: vec!["private_rule".into()],
        ..Default::default()
    };
    let first = import_records(&event.to_string(), "systemone-capture", &options)
        .unwrap()
        .remove(0);
    assert_eq!(first["answers"][0]["definition_redacted"], true);
    event["id"] = json!("call-2");
    event["request"]["questions"]["route"]["private_rule"] = json!("secret-rule-b");
    let second = import_records(&event.to_string(), "systemone-capture", &options)
        .unwrap()
        .remove(0);
    assert_ne!(
        first["answers"][0]["group_id"],
        second["answers"][0]["group_id"]
    );
    assert_ne!(first["source_event_id"], second["source_event_id"]);
    event["api_key"] = json!("outer-credential");
    event["response"]["echo"] = json!("outer-credential");
    event["request"]["questions"]["check"]["instructions"] = json!("outer-credential");
    let scrubbed = imported(&event);
    assert!(!scrubbed.to_string().contains("outer-credential"));
    assert_eq!(scrubbed["answers"][2]["definition_redacted"], true);
    event["response"]["answers"]["check"]["noul"] = json!(true);
    event["valid"] = json!(true);
    assert_eq!(imported(&event)["answers"][2]["valid"], false);
    for field in [
        "id",
        "source",
        "provider",
        "timestamp",
        "status",
        "capture_complete",
        "request",
        "response",
    ] {
        let mut bad = event.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(
            import_records(
                &format!("{}\n{}", envelope("nanojev"), bad),
                "systemone-capture",
                &options
            )
            .is_err(),
            "{field}"
        );
    }
}

#[test]
fn modal_score_preserves_provider_expected_value_and_rejects_out_of_range_indices() {
    let mut event = envelope("eikos");
    event["response"]["answers"]["rating"]["score"] = json!(1);
    event["response"]["answers"]["rating"]["expected"] = json!(0.8);
    event["response"]["answers"]["rating"]
        .as_object_mut()
        .unwrap()
        .remove("legend");
    let out = imported(&event);
    assert_eq!(out["answers"][1]["valid"], true);
    assert_eq!(out["answers"][1]["value"], 1);
    assert_eq!(out["answers"][1]["raw_answer"]["expected"], 0.8);
    for invalid in [json!(99999999), json!(-1), json!(0.8), json!(0)] {
        event["response"]["answers"]["rating"]["score"] = invalid;
        assert_eq!(imported(&event)["answers"][1]["valid"], false);
    }
}
