use super::*;

// Deliberately retain the previous exact algorithm as a differential oracle.
fn original_scrub(value: &str, secrets: &[&str]) -> String {
    let mut fragments = vec![Some(value)];
    for &secret in secrets {
        if secret.is_empty() {
            continue;
        }
        let mut next = Vec::new();
        for fragment in fragments {
            let Some(text) = fragment else {
                next.push(None);
                continue;
            };
            let mut start = 0;
            for (index, _) in text.match_indices(secret) {
                if start < index {
                    next.push(Some(&text[start..index]));
                }
                next.push(None);
                start = index + secret.len();
            }
            if start < text.len() {
                next.push(Some(&text[start..]));
            }
        }
        fragments = next;
    }
    fragments
        .into_iter()
        .map(|part| part.unwrap_or(REDACTED))
        .collect()
}

fn limit_error(error: anyhow::Error) {
    assert_eq!(
        error.downcast_ref::<ImportLimitError>().unwrap().0,
        "redaction work"
    );
}

#[test]
fn matching_preserves_exact_output_across_unicode_overlap_and_marker_corpus() {
    let alphabet = ["a", "b", "c", "é", "秘密", "E", "D", "[", "]", " "];
    let mut seed = 0x9e37_79b9_u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (seed >> 32) as usize
    };
    for case in 0..1500 {
        let value = (0..next() % 80)
            .map(|_| alphabet[next() % alphabet.len()])
            .collect::<String>();
        let mut secrets = (0..1 + next() % 12)
            .map(|_| {
                (0..1 + next() % 6)
                    .map(|_| alphabet[next() % alphabet.len()])
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        if case % 3 == 0 {
            secrets.extend([
                "bcd".into(),
                "abc".into(),
                "E".into(),
                "D".into(),
                "bcd".into(),
            ]);
        }
        let ordered = ordered_unique_secrets(&secrets);
        let expected = original_scrub(&value, &ordered);
        let mut matcher = SecretMatcher::new(ordered, REDACTION_LIMITS).unwrap();
        assert_eq!(
            scrub_string(&value, &mut matcher).unwrap(),
            expected,
            "case {case}"
        );
    }
    let secrets = ["bcd", "abc", "E", "D", "missing", "other"];
    let mut matcher = SecretMatcher::new(secrets.to_vec(), REDACTION_LIMITS).unwrap();
    assert!(matcher.prefilter.is_some());
    assert_eq!(
        scrub_string("abcd EDE", &mut matcher).unwrap(),
        format!("a{REDACTED} {}", REDACTED.repeat(3))
    );
}

#[test]
fn absent_secrets_do_not_multiply_retained_string_work() {
    let secrets = (0..8000)
        .map(|index| format!("private-credential-{index:08}"))
        .collect::<Vec<_>>();
    let mut matcher =
        SecretMatcher::new(ordered_unique_secrets(&secrets), REDACTION_LIMITS).unwrap();
    let start = matcher.remaining;
    for _ in 0..8000 {
        assert_eq!(
            scrub_string("retained-public-field", &mut matcher).unwrap(),
            "retained-public-field"
        );
    }
    assert_eq!(
        start - matcher.remaining,
        8000 * "retained-public-field".len()
    );
}

#[test]
fn leftmost_prefilter_bounds_nested_suffix_construction() {
    let mut secrets = vec!["a".repeat(16_384)];
    secrets.extend((1..=128).map(|length| "a".repeat(length)));
    let matcher = SecretMatcher::new(ordered_unique_secrets(&secrets), REDACTION_LIMITS).unwrap();
    // Standard overlapping semantics copies inherited outputs and uses over
    // 17 MiB for this 24 KiB corpus. An existence-only leftmost NFA stays small.
    assert!(matcher.prefilter.as_ref().unwrap().memory_usage() < 1024 * 1024);
}

#[test]
fn pattern_limits_reject_before_building_and_exact_marker_scans_are_charged() {
    let limits = RedactionLimits {
        patterns: 2,
        pattern_bytes: 4,
        work: 100,
    };
    limit_error(
        SecretMatcher::new(vec!["a", "b", "c"], limits)
            .err()
            .unwrap(),
    );
    limit_error(SecretMatcher::new(vec!["abc", "de"], limits).err().unwrap());
    let secrets = std::iter::once("a".repeat(32))
        .chain((0..1024).map(|index| format!("absent-{index:08}")))
        .collect::<Vec<_>>();
    let mut matcher = SecretMatcher::new(
        ordered_unique_secrets(&secrets),
        RedactionLimits {
            work: 250_000,
            ..REDACTION_LIMITS
        },
    )
    .unwrap();
    // The first secret leaves only 1,024 markers. Later absent secrets still
    // visit those markers, so remaining text-byte accounting alone is unsafe.
    limit_error(scrub_string(&"a".repeat(32 * 1024), &mut matcher).unwrap_err());
}

#[test]
fn redaction_work_allowance_is_shared_across_import_records() {
    let record = json!({"id":"shared-budget", "source":"fixture", "provider":"typesafe", "timestamp":1800000000000_i64, "status":200, "capture_complete":true,
        "request":{"credentials":"private-credential", "extension":"private-credential", "questions":{}}, "response":{"answers":{}}});
    let mut batch = ImportBudget {
        limits: IMPORT_LIMITS,
        work: 0,
        normalized: 0,
        redaction_remaining: REDACTION_LIMITS.work,
    };
    let options = NormalizeOptions::default();
    import_capture(
        &record,
        &options,
        &mut ImportRecordBudget {
            batch: &mut batch,
            work: 0,
        },
    )
    .unwrap();
    let charge = REDACTION_LIMITS.work - batch.redaction_remaining;
    assert!(charge > 0);
    batch.redaction_remaining = 2 * charge - 1;
    import_capture(
        &record,
        &options,
        &mut ImportRecordBudget {
            batch: &mut batch,
            work: 0,
        },
    )
    .unwrap();
    limit_error(
        import_capture(
            &record,
            &options,
            &mut ImportRecordBudget {
                batch: &mut batch,
                work: 0,
            },
        )
        .unwrap_err(),
    );
}

#[test]
fn privacy_limit_rejects_import_and_live_fallback_roundtrips_without_original_text() {
    let secret = "s".repeat(REDACTION_LIMITS.pattern_bytes + 1);
    let capture = Capture {
        id: "private-id".into(),
        timestamp: 1_800_000_000_000,
        source: "private-source".into(),
        status: 503,
        duration_ms: 125.5,
        capture_complete: true,
        transport_error: Some("private-transport-error".into()),
        request: serde_json::to_vec(&json!({"credentials":secret, "questions":{"q":{"type":"noul", "instructions":"private-rule"}}})).unwrap(),
        response: br#"{"answers":{"q":{"type":"noul","noul":0.5}}}"#.to_vec(),
        ..Default::default()
    };
    let original_request = capture.request.clone();
    let original_response = capture.response.clone();
    let record = normalize(&capture, &NormalizeOptions::default());
    assert_eq!(capture.request, original_request);
    assert_eq!(capture.response, original_response);
    assert_eq!(record["capture_complete"], false);
    assert_eq!(record["normalization_error"], REDACTION_LIMIT_MESSAGE);
    assert_eq!(record["timestamp"], capture.timestamp);
    assert_eq!(record["status"], capture.status);
    assert_eq!(record["duration_ms"], capture.duration_ms);
    assert_eq!(record["transport_error"], REDACTED);
    assert_eq!(record["answers"], json!([]));
    assert_eq!(record["source"], REDACTED);
    assert_ne!(
        record["id"],
        normalize(&capture, &NormalizeOptions::default())["id"]
    );
    assert!(!record.to_string().contains("private-"));
    assert!(record.to_string().len() < 2048);
    let imported = import_records(
        &record.to_string(),
        "observer-jsonl",
        &NormalizeOptions::default(),
    )
    .unwrap();
    for key in [
        "id",
        "timestamp",
        "status",
        "duration_ms",
        "capture_complete",
        "normalization_error",
        "answers",
    ] {
        assert_eq!(imported[0][key], record[key], "{key}");
    }
    let raw = json!({"id":"limited-import", "source":"fixture", "provider":"typesafe", "timestamp":1800000000000_i64, "status":200, "capture_complete":true,
        "request":{"credentials":secret,"questions":{}}, "response":{"answers":{}}});
    limit_error(
        import_records(
            &raw.to_string(),
            "systemone-capture",
            &NormalizeOptions::default(),
        )
        .unwrap_err(),
    );
}
