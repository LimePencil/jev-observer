use super::*;

#[test]
fn empty_and_extreme_timeline_ranges_are_bounded_without_overflow() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("empty-range.sqlite"), 7, 100).unwrap();
    for (from, to) in [(0, 0), (0, 1), (i64::MIN, i64::MAX), (i64::MAX, i64::MAX)] {
        let filter = Filter {
            window: Some("all".into()),
            from: Some(from),
            to: Some(to),
            ..Default::default()
        };
        let result = store.dashboard(&filter).unwrap();
        let bins = result["timeline"].as_array().unwrap();
        assert!(!bins.is_empty() && bins.len() <= 168);
        assert!(bins.iter().all(|bin| bin["requests"] == 0));
        assert_eq!(result["timeline_meta"]["start"], from);
        assert_eq!(result["timeline_meta"]["end"], to.saturating_add(1));
    }
}

#[test]
fn transfer_failures_dates_and_empty_timeline_bins_preserve_the_complete_range() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("outcomes.sqlite"), 30, 1000).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut failed = dashboard_record("body-aborted", now - 1_000);
    failed["transport_error"] = json!("Upstream response transfer failed");
    failed["capture_complete"] = json!(false);
    let healthy = dashboard_record("healthy", now - 200 * 3_600_000);
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[healthy, failed])
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        from: Some(now - 240 * 3_600_000),
        to: Some(now),
        ..Default::default()
    };
    let dashboard = store.dashboard(&filter).unwrap();
    assert_eq!(dashboard["summary"]["request_count"], 2);
    assert_eq!(dashboard["summary"]["error_count"], 1);
    assert_eq!(dashboard["requests"][0]["status"], 200);
    assert_eq!(dashboard["requests"][0]["failed"], true);
    let timeline = dashboard["timeline"].as_array().unwrap();
    assert!(timeline.len() <= 168);
    assert!(timeline.iter().any(|bin| bin["requests"] == 0));
    assert_eq!(
        timeline
            .iter()
            .map(|bin| bin["requests"].as_i64().unwrap())
            .sum::<i64>(),
        2
    );
    assert_eq!(
        timeline
            .iter()
            .map(|bin| bin["errors"].as_i64().unwrap())
            .sum::<i64>(),
        1
    );
    let errors = Filter {
        status: Some("error".into()),
        ..filter.clone()
    };
    assert_eq!(
        store.dashboard(&errors).unwrap()["requests"][0]["id"],
        "body-aborted"
    );
    let detail = store.group("dashboard-group", &filter).unwrap().unwrap();
    assert_eq!(detail["group"]["error_count"], 1);
    assert_eq!(
        detail["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bin| bin["errors"].as_i64().unwrap())
            .sum::<i64>(),
        1
    );
    let before = Filter {
        to: Some(now - 2_000),
        ..filter
    };
    assert_eq!(
        store.dashboard(&before).unwrap()["summary"]["request_count"],
        1
    );
}

#[test]
fn list_cursors_cover_tied_requests_and_groups_and_search_beyond_the_first_page() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("pages.sqlite"), 7, 1000).unwrap();
    let now = Utc::now().timestamp_millis();
    let records: Vec<_> = (0..235)
        .map(|index| {
            let mut record = dashboard_record(&format!("request-{index:03}"), now - index % 3);
            record["answers"][0]["group_id"] = json!(format!("group-{index:03}"));
            record["answers"][0]["key"] = json!(format!("question-{index:03}"));
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let mut filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let mut requests = std::collections::HashSet::new();
    loop {
        let page = store.dashboard(&filter).unwrap();
        assert_eq!(page["summary"]["request_count"], 235);
        for request in page["requests"].as_array().unwrap() {
            assert!(requests.insert(text(request, "id")));
        }
        filter.request_cursor = page["feed_next_cursor"].as_str().map(str::to_owned);
        if filter.request_cursor.is_none() {
            break;
        }
    }
    assert_eq!(requests.len(), 235);
    let mut groups = std::collections::HashSet::new();
    loop {
        let page = store.dashboard(&filter).unwrap();
        for group in page["groups"].as_array().unwrap() {
            assert!(groups.insert(text(group, "id")));
        }
        filter.group_cursor = page["group_next_cursor"].as_str().map(str::to_owned);
        if filter.group_cursor.is_none() {
            break;
        }
    }
    assert_eq!(groups.len(), 235);
    filter.group_search = Some("question-233".into());
    let found = store.dashboard(&filter).unwrap();
    assert_eq!(found["groups"].as_array().unwrap().len(), 1);
    assert_eq!(found["groups"][0]["id"], "group-233");
    assert_eq!(found["summary"]["request_count"], 235);
}

#[test]
fn version_metrics_count_parent_cost_once_and_keep_reviews_in_the_parent_scope() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("reviews.sqlite"), 7, 100).unwrap();
    let now = Utc::now().timestamp_millis();
    let mut record = dashboard_record("reviewed", now);
    let mut second = record["answers"][0].clone();
    second["key"] = json!("second");
    second["warnings"] = json!(["Reported score differs from displayed probabilities"]);
    record["answers"].as_array_mut().unwrap().push(second);
    let mut unlabeled = record["answers"][0].clone();
    unlabeled["key"] = json!("unlabeled");
    unlabeled["warnings"] = json!([]);
    record["answers"].as_array_mut().unwrap().push(unlabeled);
    let mut other = dashboard_record("other-source", now);
    other["source"] = json!("outside-filter");
    store
        .write_batch(&mut store.writer_connection().unwrap(), &[record, other])
        .unwrap();
    store.add_label("reviewed", "urgent", "correct").unwrap();
    store.add_label("reviewed", "second", "unknown").unwrap();
    store
        .add_label("other-source", "urgent", "incorrect")
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        source: Some("dashboard-test".into()),
        ..Default::default()
    };
    let detail = store.group("dashboard-group", &filter).unwrap().unwrap();
    let overview = store.dashboard(&filter).unwrap();
    tests::assert_overview_matches_detail(&overview["groups"][0], &detail["group"]);
    assert_eq!(detail["group"], detail["versions"][0]);
    for group in [&detail["group"], &detail["versions"][0]] {
        assert_eq!(
            group["review_counts"],
            json!({"correct":1,"incorrect":0,"unknown":1,"unlabeled":1})
        );
        assert_eq!(group["request_count"], 1);
        assert_eq!(group["cost_usd"], 0.25);
        assert_eq!(group["cost_known_requests"], 1);
        assert_eq!(group["input_tokens"], 100);
        assert_eq!(group["mean_latency_ms"], 4.0);
        assert_eq!(group["warning_count"], 1);
    }
    let empty = store
        .group(
            "dashboard-group",
            &Filter {
                source: Some("missing-source".into()),
                ..filter
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(empty["group"]["warning_count"], 0);
    assert_eq!(
        empty["group"]["review_counts"],
        json!({"correct":0,"incorrect":0,"unknown":0,"unlabeled":0})
    );
}

fn dashboard_record(id: &str, timestamp: i64) -> Value {
    json!({
        "schema_version": 1,
        "id": id,
        "timestamp": timestamp,
        "source": "dashboard-test",
        "model": "test-model",
        "status": 200,
        "duration_ms": 4.0,
        "input_tokens": 100,
        "output_tokens": 3,
        "cost_usd": 0.25,
        "cost_basis": "synthetic",
        "capture_complete": true,
        "sample": false,
        "answers": [{
            "key": "urgent",
            "kind": "noul",
            "group_id": "dashboard-group",
            "definition_id": "definition",
            "presentation_id": "presentation",
            "definition": {"type": "noul", "instructions": "Urgent?"},
            "valid": true,
            "value": 0.8
        }]
    })
}

fn assert_snapshot_counts(snapshot: &Value, expected: i64) {
    assert_eq!(snapshot["summary"]["request_count"], expected);
    assert_eq!(snapshot["summary"]["answer_count"], expected);
    assert_eq!(
        snapshot["requests"].as_array().unwrap().len(),
        expected as usize
    );
    let group_requests: i64 = snapshot["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["request_count"].as_i64().unwrap())
        .sum();
    assert_eq!(group_requests, expected);
    let timeline_requests: i64 = snapshot["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bucket| bucket["requests"].as_i64().unwrap())
        .sum();
    assert_eq!(timeline_requests, expected);
}

#[test]
fn dashboard_index_backfills_v1_history_without_inventing_import_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing-v1.sqlite");
    let now = Utc::now().timestamp_millis();
    let store = Store::open(&path, 7, 100).unwrap();
    let mut writer = store.writer_connection().unwrap();
    // Reproduce a populated v1 database created before the dashboard index.
    writer
        .execute_batch("DROP INDEX request_dashboard")
        .unwrap();
    let timed = dashboard_record("timed", now - 3_000);
    let mut imported = dashboard_record("untimed-import", now);
    imported.as_object_mut().unwrap().remove("timestamp");
    imported["imported_at"] = json!(now - 2_000);
    imported["duration_ms"] = json!(8.0);
    imported["source_event_id"] = json!("original-event");
    imported["import_format"] = json!("observer-jsonl");
    let mut action = dashboard_record("untimed-action", now);
    action["timestamp"] = Value::Null;
    action["imported_at"] = json!(now - 1_000);
    action["event_kind"] = json!("action");
    action["answers"] = json!([]);
    store
        .write_batch(&mut writer, &[timed, imported, action])
        .unwrap();
    store
        .add_label("untimed-import", "urgent", "correct")
        .unwrap();
    let original = store.request("untimed-import").unwrap().unwrap();
    drop(writer);
    drop(store);

    let reopened = Store::open(&path, 7, 100).unwrap();
    let connection = reopened.reader().unwrap();
    let version: i64 = connection
        .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let index_count: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name='request_dashboard'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(index_count, 1);
    assert_eq!(
        reopened.request("untimed-import").unwrap().unwrap(),
        original
    );

    let snapshot = reopened
        .dashboard(&Filter {
            window: Some("all".into()),
            as_of: Some(now),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(snapshot["summary"]["request_count"], 2);
    assert_eq!(snapshot["summary"]["answer_count"], 2);
    assert_eq!(snapshot["summary"]["action_count"], 1);
    assert_eq!(snapshot["summary"]["cost_usd"], 0.5);
    assert_eq!(snapshot["summary"]["p50_ms"], 4.0);
    assert_eq!(snapshot["summary"]["p95_ms"], 8.0);
    assert_eq!(snapshot["groups"][0]["request_count"], 2);
    assert_eq!(snapshot["groups"][0]["last_seen"], now - 2_000);
    assert_eq!(snapshot["requests"].as_array().unwrap().len(), 3);
    let timeline_requests: i64 = snapshot["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|bucket| bucket["requests"].as_i64().unwrap())
        .sum();
    assert_eq!(
        timeline_requests, 1,
        "Import time must not become provider event time"
    );
    let imported = snapshot["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|request| request["id"] == "untimed-import")
        .unwrap();
    assert!(imported["timestamp"].is_null());
    assert_eq!(imported["imported_at"], now - 2_000);
}

#[test]
fn dashboard_snapshots_follow_imports_retention_and_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("changing-history.sqlite"), 7, 2).unwrap();
    let mut writer = store.writer_connection().unwrap();
    let now = Utc::now().timestamp_millis();
    let filter = Filter {
        window: Some("all".into()),
        search: Some("urgent".into()),
        as_of: Some(now),
        ..Default::default()
    };
    let mut first = dashboard_record("first-import", now - 3_000);
    first["source_event_id"] = json!("original-event");
    first["import_format"] = json!("observer-jsonl");
    assert_eq!(store.write_batch(&mut writer, &[first.clone()]).unwrap(), 1);
    assert_snapshot_counts(&store.dashboard(&filter).unwrap(), 1);

    let mut duplicate = first;
    duplicate["id"] = json!("duplicate-import");
    let second = dashboard_record("second", now - 2_000);
    assert_eq!(
        store
            .write_batch(&mut writer, &[duplicate, second])
            .unwrap(),
        1
    );
    assert_snapshot_counts(&store.dashboard(&filter).unwrap(), 2);

    let mut third = dashboard_record("third", now - 1_000);
    third["answers"][0]["value"] = json!(0.2);
    store.write_batch(&mut writer, &[third]).unwrap();
    store.maintenance(&mut writer, true).unwrap();
    let retained = store.dashboard(&filter).unwrap();
    assert_snapshot_counts(&retained, 2);
    assert_eq!(retained["summary"]["cost_usd"], 0.5);
    assert!(store.request("first-import").unwrap().is_none());
    assert_eq!(
        retained["groups"][0]["distribution"],
        json!([
            {"label": "0.2–<0.3", "count": 1},
            {"label": "0.8–<0.9", "count": 1}
        ])
    );

    store.delete_all().unwrap();
    let empty = store.dashboard(&filter).unwrap();
    assert_snapshot_counts(&empty, 0);
    assert!(empty["summary"]["cost_usd"].is_null());
    assert!(empty["summary"]["p50_ms"].is_null());
    assert!(empty["sources"].as_array().unwrap().is_empty());
    assert!(empty["models"].as_array().unwrap().is_empty());

    store
        .write_batch(&mut writer, &[dashboard_record("after-delete", now)])
        .unwrap();
    assert_snapshot_counts(&store.dashboard(&filter).unwrap(), 1);
}

fn sql_request_aggregates(
    store: &Store,
    filter: &Filter,
    timeline_meta: &Value,
) -> (Value, Vec<Value>) {
    let connection = store.reader().unwrap();
    let (predicate, args) = Store::predicate(filter).unwrap();
    let mut summary = connection.query_row(
        &format!("SELECT count(*),coalesce(sum(answer_count),0),coalesce(sum(status>=400),0),sum(input_tokens),sum(output_tokens),sum(cost_usd),count(cost_usd),coalesce(sum(sample),0),coalesce(sum(NOT capture_complete),0) FROM requests r WHERE {predicate} AND event_kind='request'"),
        params_from_iter(&args),
        |row| Ok(json!({
            "request_count": row.get::<_, i64>(0)?,
            "answer_count": row.get::<_, i64>(1)?,
            "error_count": row.get::<_, i64>(2)?,
            "input_tokens": row.get::<_, Option<i64>>(3)?,
            "output_tokens": row.get::<_, Option<i64>>(4)?,
            "cost_usd": row.get::<_, Option<f64>>(5)?,
            "cost_known_requests": row.get::<_, i64>(6)?,
            "sample_count": row.get::<_, i64>(7)?,
            "incomplete_count": row.get::<_, i64>(8)?
        })),
    ).unwrap();
    summary["action_count"] =
        json!(connection.query_row(
        &format!("SELECT count(*) FROM requests r WHERE {predicate} AND event_kind<>'request'"),
        params_from_iter(&args),
        |row| row.get::<_, i64>(0),
    ).unwrap());
    let durations: Vec<f64> = connection.prepare(
        &format!("SELECT duration_ms FROM requests r WHERE {predicate} AND event_kind='request' AND duration_ms IS NOT NULL ORDER BY duration_ms"),
    ).unwrap().query_map(params_from_iter(&args), |row| row.get(0))
        .unwrap().collect::<rusqlite::Result<_>>().unwrap();
    for (name, numerator) in [("p50_ms", 50), ("p95_ms", 95)] {
        summary[name] = if durations.is_empty() {
            Value::Null
        } else {
            // Integer nearest rank is independent of the production selection.
            json!(durations[(durations.len() * numerator).div_ceil(100) - 1])
        };
    }
    // SQL independently aggregates every record into the response's declared
    // bins. Separate tests assert full-range coverage, empty bins and bounds.
    let width = timeline_meta["bucket_width"].as_i64().unwrap();
    let start = timeline_meta["start"].as_i64().unwrap();
    let end = timeline_meta["end"].as_i64().unwrap() - 1;
    let mut timeline: BTreeMap<i64, Value> = connection.prepare(
        &format!("SELECT ((timestamp-{start})/{width})*{width}+{start},count(*),coalesce(sum({FAILED}),0),avg(duration_ms),sum(cost_usd) FROM requests r WHERE {predicate} AND event_kind='request' AND json_extract(data,'$.timestamp') IS NOT NULL GROUP BY 1 ORDER BY 1"),
    ).unwrap().query_map(params_from_iter(&args), |row| Ok((row.get::<_, i64>(0)?, json!({
        "timestamp": row.get::<_, i64>(0)?,
        "requests": row.get::<_, i64>(1)?,
        "errors": row.get::<_, i64>(2)?,
        "mean_latency_ms": row.get::<_, Option<f64>>(3)?,
        "cost_usd": row.get::<_, Option<f64>>(4)?
    })))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    let timeline = (start..=end)
        .step_by(width as usize)
        .map(|timestamp| {
            timeline.remove(&timestamp).unwrap_or_else(|| json!({
        "timestamp":timestamp,"requests":0,"errors":0,"mean_latency_ms":null,"cost_usd":null
    }))
        })
        .collect();
    (summary, timeline)
}

#[test]
fn request_aggregates_match_sql_across_filters_unknowns_and_long_timelines() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("aggregate-reference.sqlite"), 30, 2_000).unwrap();
    let hour = 3_600_000;
    let anchor = Utc::now().timestamp_millis() / hour * hour - hour;
    let as_of = anchor + hour / 2;
    let mut records = Vec::new();
    // A coprime permutation deliberately separates insertion and timestamp
    // order. Search filters also exercise materialized parent selections.
    for ordinal in 0..193 {
        let bucket = ordinal * 73 % 193;
        for within in [2, 0, 1] {
            let number = bucket * 3 + within;
            let timestamp = anchor - bucket * hour + within * 10_000;
            let mut record = dashboard_record(&format!("record-{bucket:03}-{within}"), timestamp);
            record["source"] = json!(if bucket % 2 == 0 { "alpha" } else { "beta" });
            record["model"] = json!(if within == 1 { "model-b" } else { "model-a" });
            record["status"] = match number % 7 {
                0 => Value::Null,
                1 | 2 => json!(503),
                _ => json!(200),
            };
            record["duration_ms"] = if number % 5 == 0 {
                Value::Null
            } else {
                json!([0.1, 0.2, 0.3, 0.0, 4.125, 14.75, 101.333][number as usize % 7])
            };
            record["cost_usd"] = if number % 4 == 0 {
                Value::Null
            } else {
                json!([0.1, 0.01, 0.2, 0.0001][number as usize % 4])
            };
            record["input_tokens"] = if number % 4 == 0 {
                Value::Null
            } else {
                json!(number % 19)
            };
            record["output_tokens"] = if number % 3 == 0 {
                Value::Null
            } else {
                json!(number % 13)
            };
            record["sample"] = json!(number % 9 == 0);
            record["capture_complete"] = json!(number % 6 != 0);
            record["answers"][0]["group_id"] = json!(if bucket % 2 == 0 {
                "alpha-group"
            } else {
                "beta-group"
            });
            if within == 1 && bucket % 4 == 0 {
                let mut companion = record["answers"][0].clone();
                companion["key"] = json!("companion");
                companion["group_id"] = json!("companion-group");
                record["answers"].as_array_mut().unwrap().push(companion);
            }
            if number % 17 == 0 {
                record["timestamp"] = Value::Null;
                record["imported_at"] = json!(timestamp);
                if within == 1 {
                    record.as_object_mut().unwrap().remove("timestamp");
                }
            }
            if within == 2 && bucket % 7 == 0 {
                record["event_kind"] = json!("action");
                // Actions deliberately contain tempting metrics that must not
                // enter request totals, percentiles, or timeline averages.
                record["duration_ms"] = json!(999_999.0);
                record["cost_usd"] = json!(999.0);
                record["answers"] = json!([]);
            }
            records.push(record);
        }
    }
    records.push(dashboard_record("future", as_of + 1));
    let mut unknown = dashboard_record("unknown", anchor + 1);
    unknown["source"] = json!("unknown-only");
    for key in ["duration_ms", "cost_usd", "input_tokens", "output_tokens"] {
        unknown[key] = Value::Null;
    }
    records.push(unknown);
    let mut action = dashboard_record("action-only", anchor + 2);
    action["source"] = json!("actions-only");
    action["event_kind"] = json!("action");
    records.push(action);
    let mut source_match = dashboard_record("source-match", anchor + 3);
    source_match["source"] = json!("MiXeD-SeArCh archive");
    records.push(source_match);
    records.push(dashboard_record("mixed-search-id", anchor + 3));
    let mut key_match = dashboard_record("key-match", anchor + 3);
    let mut companion = key_match["answers"][0].clone();
    key_match["answers"][0]["key"] = json!("mixed-search-key");
    key_match["answers"][0]["group_id"] = json!("mixed-search-group");
    companion["key"] = json!("unrelated-companion");
    companion["group_id"] = json!("mixed-companion-group");
    key_match["answers"].as_array_mut().unwrap().push(companion);
    records.push(key_match);
    assert!(
        records.len() > 512,
        "Broad searches must exercise batched answer-key selection"
    );
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();

    let cases = [
        json!({"window": "all"}),
        json!({"window": "all", "search": "record"}),
        json!({"window": "all", "search": "urgent"}),
        json!({"window": "all", "search": "MiXeD-SeArCh"}),
        json!({"window": "all", "source": "alpha"}),
        json!({"window": "all", "model": "model-b"}),
        json!({"window": "all", "status": "error"}),
        json!({"window": "all", "group": "alpha-group"}),
        json!({"window": "all", "search": "companion"}),
        json!({"window": "all", "search": "record-192-1"}),
        json!({"window": "all", "search": "absent"}),
        json!({"window": "all", "source": "unknown-only"}),
        json!({"window": "all", "source": "actions-only"}),
        json!({"window": "1h"}),
        json!({"window": "24h"}),
        json!({"window": "7d"}),
        json!({"window": "24h", "source": "alpha", "model": "model-b", "status": "error", "search": "urgent"}),
        json!({}),
    ];
    for case in cases {
        let mut filter: Filter = serde_json::from_value(case.clone()).unwrap();
        filter.as_of = Some(as_of);
        let snapshot = store.dashboard(&filter).unwrap();
        let (summary, timeline) =
            sql_request_aggregates(&store, &filter, &snapshot["timeline_meta"]);
        assert_eq!(snapshot["summary"], summary, "summary for {case}");
        assert_eq!(snapshot["timeline"], json!(timeline), "timeline for {case}");
        let (predicate, args) = Store::predicate(&filter).unwrap();
        let feed =
            Store::request_summaries(&store.reader().unwrap(), &predicate, &args, 100).unwrap();
        assert_eq!(
            snapshot["requests"],
            json!(feed),
            "newest-first feed for {case}"
        );
        if case["search"] == "MiXeD-SeArCh" {
            assert_eq!(snapshot["summary"]["request_count"], 3);
            assert_eq!(snapshot["summary"]["answer_count"], 4);
            let companion = snapshot["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|group| group["id"] == "mixed-companion-group")
                .unwrap();
            assert_eq!(
                companion["answer_count"], 1,
                "A key match must retain the parent's other answers"
            );
            assert_eq!(snapshot["requests"][0]["id"], "key-match");
            assert_eq!(snapshot["requests"][1]["id"], "mixed-search-id");
            assert_eq!(snapshot["requests"][2]["id"], "source-match");
        }
        if case == json!({"window": "all"}) {
            assert!(snapshot["timeline"].as_array().unwrap().len() <= 168);
            assert_eq!(snapshot["timeline_meta"]["start"], anchor - 192 * hour);
            assert_eq!(snapshot["timeline_meta"]["end"], as_of + 2);
            assert_eq!(snapshot["timeline_meta"]["truncated"], false);
        }
    }
}

#[test]
fn request_aggregates_preserve_small_fractional_charges_next_to_large_values() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("compensated-metrics.sqlite"), 7, 2_000).unwrap();
    let timestamp = Utc::now().timestamp_millis();
    let mut records = Vec::new();
    for ordinal in 0..1_001 {
        let mut record = dashboard_record(&format!("fraction-{ordinal:04}"), timestamp);
        let value = if ordinal == 0 { 1e12 } else { 0.1 };
        record["cost_usd"] = json!(value);
        record["duration_ms"] = json!(value);
        records.push(record);
    }
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let snapshot = store.dashboard(&filter).unwrap();
    let (summary, timeline) = sql_request_aggregates(&store, &filter, &snapshot["timeline_meta"]);
    assert_eq!(snapshot["summary"], summary);
    assert_eq!(snapshot["timeline"], json!(timeline));
    // An ordinary left-to-right floating sum loses about 0.024 here.
    assert_eq!(snapshot["summary"]["cost_usd"], json!(1_000_000_000_100.0));
    assert_eq!(
        snapshot["timeline"][0]["cost_usd"],
        json!(1_000_000_000_100.0)
    );
    assert_eq!(
        snapshot["timeline"][0]["mean_latency_ms"],
        json!(1_000_000_000_100.0 / 1_001.0)
    );
    assert_eq!(snapshot["summary"]["p50_ms"], 0.1);
    assert_eq!(snapshot["summary"]["p95_ms"], 0.1);
}

#[test]
#[ignore = "bounded CSV export timing; run explicitly with --ignored --nocapture"]
fn csv_export_large_definitions_benchmark() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("csv-definitions.sqlite"), 36_500, 1_000).unwrap();
    let records: Vec<_> = (0..200)
        .map(|ordinal| {
            let mut record = dashboard_record(&format!("csv-{ordinal:03}"), 1_790_000_000_000);
            let answers: Vec<_> = (0..10)
                .map(|key| {
                    let mut answer = record["answers"][0].clone();
                    answer["key"] = json!(format!("key-{key}"));
                    answer["group_id"] = json!(format!("group-{key}"));
                    answer["definition"]["instructions"] = json!("x".repeat(8 * 1024));
                    answer
                })
                .collect();
            record["answers"] = json!(answers);
            record
        })
        .collect();
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let filter = Filter {
        window: Some("all".into()),
        ..Default::default()
    };
    let expected = store.export(&filter, "csv").unwrap();
    assert_eq!(expected.lines().count(), 201);
    let mut elapsed = Vec::new();
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let csv = store.export(&filter, "csv").unwrap();
        elapsed.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(csv, expected);
    }
    use sha2::{Digest, Sha256};
    println!(
        "{}",
        json!({
            "fixture": {"requests": 200, "answers_per_request": 10, "definition_bytes": 8_192},
            "csv_bytes": expected.len(),
            "csv_sha256": format!("{:x}", Sha256::digest(expected.as_bytes())),
            "elapsed_ms": elapsed,
        })
    );
}
