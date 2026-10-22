use super::*;

fn decision_capture(choice: Value) -> (Capture, NormalizeOptions) {
    let boolean = choice.is_boolean();
    (Capture {
        id: CaptureId::new_live(), timestamp: chrono::Utc::now().timestamp_millis(),
        source: "typed-fixture".into(), status: 200, capture_complete: true,
        request: serde_json::to_vec(&json!({"model":"gpt-6-luna","input":{"api_key":"private-input-token","text":"private input"},
            "questions":[{"type":"choice","name":"same","instructions":"Choose","choices":[{"value":true},{"value":"true"}]}]})).unwrap(),
        response: serde_json::to_vec(&json!({"model":"gpt-6-luna","answers":[{"type":"choice","name":"same","choice":choice,"confidence":0.7,
            "probabilities":[{"value":true,"probability":if boolean {0.8}else{0.2}},{"value":"true","probability":if boolean {0.2}else{0.8}}]}],
            "usage":{"input_tokens":12,"output_tokens":0,"total_tokens":12,"input_tokens_details":{"cached_tokens":0}}})).unwrap(),
        ..Default::default()
    }, NormalizeOptions {provider:Some("openai".into()),protocol:Protocol::OpenaiDecisions,..Default::default()})
}

#[test]
fn typed_choices_survive_normalization_storage_grouping_and_jsonl() {
    let records: Vec<_> = [json!(true), json!("true")]
        .into_iter()
        .map(|choice| {
            let (capture, options) = decision_capture(choice);
            let record = normalize(&capture, &options);
            assert_eq!(record["answers"][0]["valid"], true, "{record}");
            assert!(!record.to_string().contains("private-input"));
            let imported = import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default(),
            )
            .unwrap()
            .remove(0);
            assert_eq!(imported["answers"], record["answers"]);
            record
        })
        .collect();
    assert_ne!(
        records[0]["answers"][0]["choice_key"],
        records[1]["answers"][0]["choice_key"]
    );
    assert_eq!(
        records[0]["answers"][0]["group_id"],
        records[1]["answers"][0]["group_id"]
    );
    let temp = tempfile::tempdir().unwrap();
    let store = crate::store::Store::open_demo(&temp.path().join("typed.sqlite"), 7, 1000).unwrap();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let filter = crate::store::Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let dashboard = store.dashboard(&filter).unwrap();
    let group = &dashboard["groups"][0];
    assert_eq!(group["valid_count"], 2);
    assert_eq!(group["distribution"].as_array().unwrap().len(), 2);
    let bins = group["distribution"].as_array().unwrap();
    assert!(
        bins.iter()
            .any(|bin| bin["value"] == true && bin["count"] == 1)
    );
    assert!(
        bins.iter()
            .any(|bin| bin["value"] == "true" && bin["count"] == 1)
    );
    let detail = store
        .group(group["id"].as_str().unwrap(), &filter)
        .unwrap()
        .unwrap();
    assert_eq!(detail["group"]["distribution"], group["distribution"]);
    assert!(detail["answers"][0]["choice_key"].is_string());
    assert_eq!(
        store
            .request(records[0]["id"].as_str().unwrap())
            .unwrap()
            .unwrap()["answers"][0]["value"],
        true
    );
}

#[test]
fn decisions_order_handles_duplicate_names_and_refusals_without_rekeying() {
    let (mut capture, options) = decision_capture(json!(true));
    capture.request = serde_json::to_vec(&json!({"input":"input","questions":[
        {"type":"predicate","name":"same","instructions":"First"},
        {"type":"predicate","name":"same","instructions":"Second"},
        {"type":"predicate","instructions":"Third"}]}))
    .unwrap();
    capture.response = serde_json::to_vec(
        &json!({"answers":[{"type":"predicate","name":"same","probability":0.8},
        {"type":"refusal","name":"same"},{"type":"predicate","name":null,"probability":0.4}]}),
    )
    .unwrap();
    let record = normalize(&capture, &options);
    let answers = record["answers"].as_array().unwrap();
    assert_eq!(answers.len(), 3);
    assert_eq!(answers[0]["key"], "question:0");
    assert_eq!(answers[0]["value"], 0.8);
    assert_eq!(answers[1]["valid"], false);
    assert!(answers[1]["value"].is_null());
    assert!(answers[1]["error"].as_str().unwrap().contains("refused"));
    assert_eq!(answers[2]["valid"], true);
    assert_eq!(
        import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default()
        )
        .unwrap()[0]["answers"],
        record["answers"]
    );
}

#[test]
fn decisions_reject_wrong_shapes_typed_duplicates_and_mismatched_names() {
    let (original, options) = decision_capture(json!(true));
    for mutation in 0..4 {
        let mut capture = original.clone();
        let mut request: Value = serde_json::from_slice(&capture.request).unwrap();
        let mut response: Value = serde_json::from_slice(&capture.response).unwrap();
        match mutation {
            0 => request["questions"] = json!({"q":{"type":"predicate","instructions":"Bad wire"}}),
            1 => response["answers"][0]["probabilities"][1]["value"] = json!(true),
            2 => response["answers"][0]["name"] = json!("wrong"),
            _ => response["answers"] = json!([]),
        }
        capture.request = serde_json::to_vec(&request).unwrap();
        capture.response = serde_json::to_vec(&response).unwrap();
        let record = normalize(&capture, &options);
        assert!(
            record["answers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|answer| answer["valid"] == false),
            "{mutation}: {record}"
        );
        assert!(
            record["normalization_error"].is_string()
                || record["answers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|answer| answer["valid"] == false)
        );
    }
}

#[test]
fn decisions_score_keeps_native_levels_and_validates_indices_and_labels() {
    let levels: Vec<_> = (0..12)
        .map(|index| json!({"label":format!("L{index}")}))
        .collect();
    let definition = json!({"type":"score","instructions":"Rate","levels":levels});
    let mut raw = json!({"type":"score","score":11.0,"confidence":0.9,"probabilities":(0..12).map(|index|json!({"value":index,"label":format!("L{index}"),"probability":if index==11 {1.0}else{0.0}})).collect::<Vec<_>>()});
    assert!(super::decisions::validate(&definition, &raw, &mut Vec::new()).is_empty());
    raw["probabilities"][11]["label"] = json!("wrong");
    assert!(!super::decisions::validate(&definition, &raw, &mut Vec::new()).is_empty());
}

#[test]
fn decisions_privacy_detects_typed_collisions_and_cannot_repair_descriptions() {
    let (mut capture, mut options) = decision_capture(json!(true));
    let mut request: Value = serde_json::from_slice(&capture.request).unwrap();
    request["questions"][0]["choices"] =
        json!([{"value":"private-input-token"},{"value":"[REDACTED]"}]);
    capture.request = serde_json::to_vec(&request).unwrap();
    let record = normalize(&capture, &options);
    assert_eq!(record["capture_complete"], false);
    assert!(!record.to_string().contains("private-input-token"));
    let (mut capture, _) = decision_capture(json!(true));
    let mut request: Value = serde_json::from_slice(&capture.request).unwrap();
    request["questions"][0]["choices"][0]["description"] = Value::Null;
    capture.request = serde_json::to_vec(&request).unwrap();
    options.redact_keys = vec!["description".into()];
    assert_eq!(normalize(&capture, &options)["capture_complete"], false);
}

#[test]
fn decisions_privacy_preserves_original_name_equality_on_capture_and_import() {
    for matching in [false, true] {
        let request = json!({"input":{"api_key":"fixture-name-A","authorization":"Bearer fixture-name-B"},
            "questions":[{"type":"predicate","name":"fixture-name-A","instructions":"Check"}]});
        let response = json!({"answers":[{"type":"predicate","name":if matching {"fixture-name-A"} else {"fixture-name-B"},"probability":0.8}]});
        let options = NormalizeOptions {
            protocol: Protocol::OpenaiDecisions,
            ..Default::default()
        };
        let capture = Capture {
            id: CaptureId::new_live(),
            source: "privacy-fixture".into(),
            status: 200,
            capture_complete: true,
            request: serde_json::to_vec(&request).unwrap(),
            response: serde_json::to_vec(&response).unwrap(),
            ..Default::default()
        };
        let record = normalize(&capture, &options);
        assert_eq!(record["capture_complete"], matching, "{record}");
        assert!(!record.to_string().contains("fixture-name-"));
        let envelope = json!({"id":"privacy-native","source":"privacy-native","provider":"openai","timestamp":1,"capture_complete":true,"protocol":"openai-decisions","status":200,"request":request,"response":response});
        let imported = import_records(&envelope.to_string(), "systemone-capture", &options);
        assert_eq!(imported.is_ok(), matching, "{imported:?}");
        if matching {
            assert_eq!(record["answers"][0]["valid"], true);
            let roundtrip = import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default(),
            )
            .unwrap();
            assert_eq!(roundtrip[0]["answers"], record["answers"]);
        }
    }
}

#[test]
fn decisions_privacy_cannot_repair_score_labels_and_keeps_duplicate_levels() {
    for label in [json!(123), json!("Wrong"), json!("Low")] {
        let request = json!({"input":"sample","questions":[{"type":"score","instructions":"Rate","levels":[{"label":"Low"},{"label":"High"}]}]});
        let response = json!({"answers":[{"type":"score","score":0.25,"confidence":0.8,"probabilities":[{"value":0,"label":label,"probability":0.75},{"value":1,"label":"High","probability":0.25}]}]});
        let options = NormalizeOptions {
            protocol: Protocol::OpenaiDecisions,
            redact_keys: vec!["label".into()],
            ..Default::default()
        };
        let mut capture = Capture {
            id: CaptureId::new_live(),
            source: "privacy-fixture".into(),
            status: 200,
            capture_complete: true,
            request: serde_json::to_vec(&request).unwrap(),
            response: serde_json::to_vec(&response).unwrap(),
            ..Default::default()
        };
        let record = normalize(&capture, &options);
        let valid = label == "Low";
        assert_eq!(record["capture_complete"], valid, "{label}: {record}");
        let envelope = json!({"id":"privacy-native","source":"privacy-native","provider":"openai","timestamp":1,"capture_complete":true,"protocol":"openai-decisions","status":200,"request":request,"response":response});
        assert_eq!(
            import_records(&envelope.to_string(), "systemone-capture", &options).is_ok(),
            valid
        );
        if valid {
            assert_eq!(record["answers"][0]["valid"], true);
            let roundtrip = import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default(),
            )
            .unwrap();
            assert_eq!(roundtrip[0]["answers"], record["answers"]);
            let mut duplicate_request = request;
            let mut duplicate_response = response;
            duplicate_request["questions"][0]["levels"][1]["label"] = json!("Low");
            duplicate_response["answers"][0]["probabilities"][1]["label"] = json!("Low");
            capture.request = serde_json::to_vec(&duplicate_request).unwrap();
            capture.response = serde_json::to_vec(&duplicate_response).unwrap();
            assert_eq!(normalize(&capture, &options)["answers"][0]["valid"], true);
        }
    }
}

#[test]
fn decisions_rounded_selection_difference_is_a_warning_without_rewriting_values() {
    let (mut capture, options) = decision_capture(json!(true));
    let mut response: Value = serde_json::from_slice(&capture.response).unwrap();
    response["answers"][0]["probabilities"][0]["probability"] = json!(0.2);
    response["answers"][0]["probabilities"][1]["probability"] = json!(0.8);
    capture.response = serde_json::to_vec(&response).unwrap();
    let record = normalize(&capture, &options);
    assert_eq!(record["answers"][0]["valid"], true);
    assert_eq!(record["answers"][0]["value"], true);
    assert!(
        record["answers"][0]["warnings"]
            .to_string()
            .contains("highest displayed")
    );
    assert_eq!(record["answers"][0]["raw_answer"], response["answers"][0]);
}

#[test]
fn decisions_wire_question_limit_keeps_the_documented_boundary() {
    let definition = json!({"type":"predicate","instructions":"Check"});
    let answer = json!({"type":"predicate","probability":0.8});
    for size in [0, 400, 401] {
        let request = json!({"questions":vec![definition.clone();size]});
        let response = json!({"answers":vec![answer.clone();size]});
        assert_eq!(
            super::decisions::valid_wire_shape(&request, &response, true),
            size == 400
        );
    }
}

#[test]
fn box_validation_respects_geometry_presence_and_profile() {
    let definition = json!({"type":"box","instructions":"Find object"});
    let answer = json!({"type":"box","box":{"top":0.1,"bottom":0.8,"left":0.2,"right":0.9},"confidence":0.7});
    let profile = catalog::Profile {
        box_answers: true,
        box_presence: true,
        ..Default::default()
    };
    assert!(
        super::validation::validate_answer(&definition, &answer, "box", &profile, &mut Vec::new())
            .is_empty()
    );
    for (field, value) in [
        ("top", json!(0.8)),
        ("left", json!(-0.1)),
        ("right", json!("0.9")),
        ("bottom", Value::Null),
    ] {
        let mut invalid = answer.clone();
        invalid["box"][field] = value;
        assert!(
            !super::validation::validate_answer(
                &definition,
                &invalid,
                "box",
                &profile,
                &mut Vec::new()
            )
            .is_empty()
        );
    }
    let mut missing = answer.clone();
    missing["found"] = json!(false);
    missing["present"] = json!(0.1);
    let mut warnings = Vec::new();
    assert!(
        super::validation::validate_answer(&definition, &missing, "box", &profile, &mut warnings)
            .is_empty()
    );
    assert!(warnings[0].contains("does not establish"));
    assert!(
        !super::validation::validate_answer(
            &definition,
            &answer,
            "box",
            &catalog::Profile::default(),
            &mut Vec::new()
        )
        .is_empty()
    );
}

#[test]
fn cloudflare_wrapper_is_validated_and_media_opt_out_survives_reimport() {
    let capture=Capture{id:CaptureId::new_live(),source:"cloudflare".into(),status:200,capture_complete:true,
        request:serde_json::to_vec(&json!({"questions":{"q":{"type":"noul","instructions":"Check"}},"assets":{"image":{"api_key":"asset-secret","data_base64":"input-bytes"}}})).unwrap(),
        response:serde_json::to_vec(&json!({"success":true,"errors":[],"messages":[],"result":{"model":"clef-omni","answers":{"q":{"type":"noul","noul":0.8}},"media":["output-input"],"note":"asset-secret"}})).unwrap(),..Default::default()};
    let options = NormalizeOptions {
        protocol: Protocol::CloudflareSystemone,
        ..Default::default()
    };
    let record = normalize(&capture, &options);
    assert_eq!(record["answers"][0]["valid"], true, "{record}");
    for secret in ["asset-secret", "input-bytes", "output-input"] {
        assert!(!record.to_string().contains(secret));
    }
    assert_eq!(record["response_extra"]["note"], "[REDACTED]");
    assert_eq!(record["protocol_metadata"]["success"], true);
    let roundtrip = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0);
    assert_eq!(roundtrip["answers"], record["answers"]);
    assert_eq!(roundtrip["protocol_metadata"], record["protocol_metadata"]);
    let mut failed = capture;
    let mut response: Value = serde_json::from_slice(&failed.response).unwrap();
    response["success"] = json!(false);
    failed.response = serde_json::to_vec(&response).unwrap();
    assert_eq!(normalize(&failed, &options)["answers"][0]["valid"], false);
}

#[test]
fn question_asset_references_are_isolated_but_state_only_media_does_not_split_questions() {
    let mut capture=Capture{id:CaptureId::new_live(),source:"images".into(),status:200,capture_complete:true,
        request:serde_json::to_vec(&json!({"state":"asset-ref","assets":{"asset-ref":{"format":"png","data_base64":"sensitive-image"}},"questions":{"q":{"type":"noul","instructions":"Compare against asset-ref"},"state_only":{"type":"noul","instructions":"Is the state image bright?","criteria":{"asset-ref":null}}}})).unwrap(),
        response:serde_json::to_vec(&json!({"answers":{"q":{"type":"noul","noul":0.7},"state_only":{"type":"noul","noul":0.6}}})).unwrap(),..Default::default()};
    let first = normalize(&capture, &NormalizeOptions::default());
    capture.id = CaptureId::new_live();
    let second = normalize(&capture, &NormalizeOptions::default());
    assert_ne!(
        first["answers"][0]["group_id"],
        second["answers"][0]["group_id"]
    );
    assert_eq!(
        first["answers"][1]["group_id"],
        second["answers"][1]["group_id"]
    );
    assert!(!first.to_string().contains("sensitive-image"));
    assert_eq!(
        import_records(
            &first.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default()
        )
        .unwrap()[0]["answers"][0]["group_id"],
        first["answers"][0]["group_id"]
    );
}

#[test]
fn explicit_model_profile_does_not_replace_provider_cost_attribution() {
    let capture = Capture {
        id: CaptureId::new_live(),
        source: "gateway".into(),
        status: 200,
        capture_complete: true,
        request: serde_json::to_vec(
            &json!({"questions":{"q":{"type":"noul","instructions":"Check"}}}),
        )
        .unwrap(),
        response: serde_json::to_vec(
            &json!({"answers":{"q":{"type":"noul","noul":0.7}},"usage":{"cost":0.25}}),
        )
        .unwrap(),
        ..Default::default()
    };
    let options = NormalizeOptions {
        provider: Some("openrouter".into()),
        model_profile: Some("laya".into()),
        ..Default::default()
    };
    let record = normalize(&capture, &options);
    assert_eq!(record["cost_usd"], 0.25);
    assert_eq!(record["provider"], "openrouter");
    assert_eq!(record["model_profile"], "laya");
    let roundtrip = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0);
    assert_eq!(roundtrip["answers"], record["answers"]);
    let custom = normalize(
        &capture,
        &NormalizeOptions {
            provider: Some("custom".into()),
            model_profile: Some("openrouter".into()),
            ..Default::default()
        },
    );
    assert!(custom["cost_usd"].is_null());
}

#[test]
fn abstention_mass_and_invalid_flags_never_pass_rounding_tolerance() {
    let definition = json!({"type":"choice","instructions":"Pick","criteria":{"a":null,"b":null}});
    let profile = catalog::Profile {
        probability_decimals: Some(4),
        reported_abstention: true,
        ..Default::default()
    };
    let mut raw = json!({"type":"choice","choice":"a","probabilities":{"a":0.99995,"b":0.0},"confidence":1.0,"valid":true,"abstain":0.00005});
    assert!(
        !super::validation::validate_answer(&definition, &raw, "choice", &profile, &mut Vec::new())
            .is_empty()
    );
    raw["abstain"] = json!(0.0);
    raw["valid"] = json!(false);
    assert!(
        !super::validation::validate_answer(&definition, &raw, "choice", &profile, &mut Vec::new())
            .is_empty()
    );
}

#[test]
fn box_observations_keep_geometry_and_reviews_without_scalar_histograms() {
    let temp = tempfile::tempdir().unwrap();
    let store = crate::store::Store::open_demo(&temp.path().join("boxes.sqlite"), 7, 2000).unwrap();
    let options = NormalizeOptions {
        model_profile: Some("h2o-lightning".into()),
        ..Default::default()
    };
    let mut capture=Capture{id:CaptureId::new_live(),timestamp:chrono::Utc::now().timestamp_millis(),source:"box-fixture".into(),status:200,capture_complete:true,
        request:serde_json::to_vec(&json!({"questions":{"object":{"type":"box","instructions":"Locate object"}}})).unwrap(),
        response:serde_json::to_vec(&json!({"answers":{"object":{"type":"box","box":{"top":0.0,"bottom":1.0,"left":0.0,"right":1.0},"confidence":0.0,"found":false,"present":0.1}}})).unwrap(),..Default::default()};
    let record = normalize(&capture, &options);
    assert_eq!(record["answers"][0]["valid"], true, "{record}");
    let roundtrip = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0);
    assert_eq!(roundtrip["answers"], record["answers"]);
    let mut records = vec![record.clone()];
    // Exercise both the broad strict-group covering scan and a filtered scan.
    for index in 0..600 {
        capture.id = CaptureId::new_live();
        capture.source = if index % 2 == 0 {
            "box-fixture"
        } else {
            "other-box"
        }
        .into();
        records.push(normalize(&capture, &options));
    }
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    store
        .add_label(record["id"].as_str().unwrap(), "object", "correct")
        .unwrap();
    for source in [None, Some("box-fixture".into())] {
        let filter = crate::store::Filter {
            window: Some("all".into()),
            source,
            ..Default::default()
        };
        let dashboard = store.dashboard(&filter).unwrap();
        for group in dashboard["groups"].as_array().unwrap() {
            assert!(group["mean_value"].is_null());
            assert_eq!(group["distribution"], json!([]));
            assert!(group["valid_count"].as_i64().unwrap() > 0);
        }
        let detail = store
            .group(record["answers"][0]["group_id"].as_str().unwrap(), &filter)
            .unwrap()
            .unwrap();
        assert_eq!(detail["group"]["distribution"], json!([]));
        assert!(detail["group"]["mean_value"].is_null());
        assert_eq!(detail["group"]["review_counts"]["correct"], 1);
        assert_eq!(detail["answers"][0]["box_found"], false);
        assert_eq!(detail["answers"][0]["value"]["right"], 1.0);
    }
}

#[test]
fn raw_protocol_imports_preserve_retained_inputs_and_reject_profile_repair() {
    let (capture, options) = decision_capture(json!(true));
    let envelope = json!({"id":"native-openai-call","source":"raw","provider":"openai","protocol":"openai-decisions","model_profile":"openai-decisions",
        "timestamp":capture.timestamp,"status":200,"duration_ms":null,"capture_complete":true,
        "request":serde_json::from_slice::<Value>(&capture.request).unwrap(),"response":serde_json::from_slice::<Value>(&capture.response).unwrap()});
    let retained = import_records(
        &envelope.to_string(),
        "systemone-capture",
        &NormalizeOptions {
            capture_state: true,
            ..options.clone()
        },
    )
    .unwrap()
    .remove(0);
    assert_eq!(retained["answers"][0]["valid"], true);
    assert_eq!(retained["state_retained"], true);
    assert_eq!(retained["state"]["api_key"], "[REDACTED]");
    let omitted = import_records(
        &retained.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap()
    .remove(0);
    assert_eq!(omitted["state_retained"], false);
    assert!(omitted["state"].is_null());
    assert!(!omitted.to_string().contains("private input"));
    let mut sensitive = envelope;
    sensitive["model_profile"] = json!("laya");
    sensitive["request"]["input"]["api_key"] = json!("laya");
    assert!(
        import_records(
            &sensitive.to_string(),
            "systemone-capture",
            &NormalizeOptions::default()
        )
        .is_err()
    );
}

#[test]
fn legacy_choice_labels_and_extensions_cannot_select_typed_rendering() {
    let capture=Capture{id:CaptureId::new_live(),timestamp:chrono::Utc::now().timestamp_millis(),source:"legacy".into(),status:200,capture_complete:true,
        request:serde_json::to_vec(&json!({"questions":{"q":{"type":"choice","instructions":"Choose","criteria":{"choice:v1:true":null,"choice:v1:\"true\"":null},"choices":[{"value":true},{"value":"true"}]}}})).unwrap(),
        response:serde_json::to_vec(&json!({"answers":{"q":{"type":"choice","choice":"choice:v1:true","probabilities":{"choice:v1:true":0.8,"choice:v1:\"true\"":0.2},"confidence":0.6}}})).unwrap(),..Default::default()};
    let record = normalize(&capture, &NormalizeOptions::default());
    assert_eq!(record["answers"][0]["valid"], true);
    let temp = tempfile::tempdir().unwrap();
    let store =
        crate::store::Store::open_demo(&temp.path().join("legacy.sqlite"), 7, 1000).unwrap();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[record])
        .unwrap();
    let filter = crate::store::Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let dashboard = store.dashboard(&filter).unwrap();
    let group = &dashboard["groups"][0];
    assert!(group["adapter"].is_null());
    assert_eq!(
        group["distribution"],
        json!([{"label":"choice:v1:true","count":1}])
    );
    let detail = store
        .group(group["id"].as_str().unwrap(), &filter)
        .unwrap()
        .unwrap();
    assert_eq!(detail["group"]["distribution"], group["distribution"]);
}

#[test]
fn offline_omni_converter_fixtures_revalidate_and_roundtrip_without_renormalizing() {
    let fixtures: Value = serde_json::from_str(include_str!(
        "../../../research/models/omni-converter-fixtures.json"
    ))
    .unwrap();
    for case in fixtures["cases"].as_array().unwrap() {
        let record = import_records(
            &case["capture"].to_string(),
            "systemone-capture",
            &NormalizeOptions::default(),
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            record["answers"][0]["valid"], case["valid"],
            "{}: {record}",
            case["name"]
        );
        assert_eq!(
            record["answers"][0]["raw_answer"],
            case["capture"]["response"]["answers"]["q"]
        );
        assert_eq!(record["provider"], "local-provider");
        assert_eq!(record["model_profile"], case["capture"]["model_profile"]);
        let roundtrip = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap()
        .remove(0);
        assert_eq!(roundtrip["answers"], record["answers"], "{}", case["name"]);
    }
}
