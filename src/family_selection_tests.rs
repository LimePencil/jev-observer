use super::*;
use sha2::{Digest, Sha256};

#[derive(Default, Debug)]
struct FamilyWork {
    steps: i64,
    scans: i64,
    probe_steps: i64,
    selected: bool,
    queries: Vec<String>,
}

fn measured(store: &Store, filter: &Filter) -> (Value, FamilyWork, Vec<String>) {
    struct Trace(*mut rusqlite::ffi::sqlite3);
    impl Drop for Trace {
        fn drop(&mut self) {
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(self.0, 0, None, std::ptr::null_mut());
            }
        }
    }
    unsafe extern "C" fn profile(
        _: u32,
        context: *mut std::ffi::c_void,
        statement: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i32 {
        unsafe {
            let statement = statement.cast::<rusqlite::ffi::sqlite3_stmt>();
            let sql =
                std::ffi::CStr::from_ptr(rusqlite::ffi::sqlite3_sql(statement)).to_string_lossy();
            let work = &mut *context.cast::<FamilyWork>();
            let steps = i64::from(rusqlite::ffi::sqlite3_stmt_status(
                statement,
                rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,
                1,
            ));
            if sql.starts_with("SELECT g.family_id AS display_id,")
                || sql.starts_with("SELECT display_id,bin,bin_count FROM (")
            {
                work.steps += steps;
                work.scans += i64::from(rusqlite::ffi::sqlite3_stmt_status(
                    statement,
                    rusqlite::ffi::SQLITE_STMTSTATUS_FULLSCAN_STEP,
                    1,
                ));
                work.selected |= sql.contains(FAMILY_PAIR_SELECTION);
                work.queries.push(sql.into_owned());
            } else if sql == FAMILY_PAIR_QUERY
                || sql.starts_with("INSERT OR IGNORE INTO temp.dashboard_family_pairs")
            {
                work.probe_steps += steps;
            }
        }
        0
    }
    let mut connection = store.reader().unwrap();
    let mut work = FamilyWork::default();
    unsafe {
        rusqlite::ffi::sqlite3_trace_v2(
            connection.handle(),
            rusqlite::ffi::SQLITE_TRACE_PROFILE,
            Some(profile),
            std::ptr::from_mut(&mut work).cast(),
        );
    }
    let trace = Trace(unsafe { connection.handle() });
    let result = Store::dashboard_from(&mut connection, filter).unwrap();
    drop(trace);
    assert_eq!(
        connection
            .query_row("PRAGMA temp_store", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let mut plans = Vec::new();
    // These planning checks use the actual recorded SQL and exact bindings for
    // a plain model filter. More involved scopes are checked by response bytes.
    if filter.search.is_none()
        && filter.group.is_none()
        && filter.group_cursor.is_none()
        && filter.group_search.is_none()
    {
        // dashboard_from drops its read transaction (including its TEMP
        // table) before returning. Recreate the exact selection in another
        // read transaction on this unchanged fixture solely for EXPLAIN.
        // The profile above measures only the original Store call.
        let plan_transaction = connection.transaction().unwrap();
        let (predicate, args) = Store::predicate(filter).unwrap();
        if work.selected {
            let mut statement = plan_transaction
                .prepare(&format!(
                    "SELECT id,timestamp FROM requests r WHERE {predicate}"
                ))
                .unwrap();
            let parents: HashMap<String, i64> = statement
                .query_map(params_from_iter(&args), |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert_eq!(
                Store::family_dashboard_selection(&plan_transaction, &parents).unwrap(),
                FAMILY_PAIR_SELECTION
            );
        }
        for query in &work.queries {
            let mut args = args.clone();
            if query.starts_with("SELECT display_id,bin,bin_count") {
                args.extend(
                    result["groups"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|g| g["is_family"] == true)
                        .map(|g| text(g, "id").into()),
                );
            }
            let mut statement = plan_transaction
                .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
                .unwrap();
            plans.extend(
                statement
                    .query_map(params_from_iter(args), |r| r.get::<_, String>(3))
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap(),
            );
        }
    }
    (result, work, plans)
}

fn answer(group: &str, key: &str, family: Option<&str>) -> Value {
    json!({"group_id":group,"key":key,"kind":"noul","definition_id":group,"presentation_id":group,
        "definition":{"type":"noul","instructions":group},"family_id":family,"family_name":family,
        "valid":true,"value":0.75,"confidence":0.5})
}
fn record(id: &str, model: &str, answers: Vec<Value>) -> Value {
    json!({"schema_version":1,"id":id,"timestamp":4_000_000_000_000_i64,"source":"source",
        "model":model,"status":200,"duration_ms":2.0,"input_tokens":5,"output_tokens":2,
        "capture_complete":true,"sample":false,"answers":answers})
}
fn filter() -> Filter {
    Filter {
        window: Some("all".into()),
        model: Some("needle".into()),
        as_of: Some(4_000_000_000_000),
        ..Default::default()
    }
}
fn evidence(name: &str, value: &Value, work: &FamilyWork) {
    println!(
        "FAMILY_SELECTION_EVIDENCE {name} {:x} {} {} {}",
        Sha256::digest(serde_json::to_vec(value).unwrap()),
        work.steps,
        work.scans,
        work.probe_steps
    );
}

#[test]
fn family_selection_bounds_actual_queries_for_shared_and_unrelated_groups() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("family-work.sqlite"), 7, 5000).unwrap();
    let mut writer = store.writer_connection().unwrap();
    let mut observations = Vec::new();
    let mut inserted = 0;
    for count in [128, 2048] {
        let records: Vec<_> = (inserted..count)
            .map(|index| {
                record(
                    &format!("parent-{index:04}"),
                    if index == 0 { "needle" } else { "noise" },
                    vec![
                        answer("shared-group", "shared", Some("family-shared")),
                        answer(
                            &format!("unique-{index:04}"),
                            "unique",
                            Some(&format!("family-{index:04}")),
                        ),
                        answer("strict", "strict", None),
                    ],
                )
            })
            .collect();
        store.write_batch(&mut writer, &records).unwrap();
        inserted = count;
        let (result, work, plans) = measured(&store, &filter());
        evidence(&format!("growth-{count}"), &result, &work);
        assert_eq!(result["summary"]["request_count"], 1);
        assert_eq!(result["summary"]["answer_count"], 3);
        assert_eq!(result["groups"].as_array().unwrap().len(), 3);
        for group in result["groups"].as_array().unwrap() {
            assert_eq!(group["request_count"], 1);
            assert_eq!(group["answer_count"], 1);
            assert_eq!(group["mean_value"], 0.75);
            assert_eq!(
                group["distribution"],
                json!([{"label":"0.7–<0.8","count":1}])
            );
        }
        observations.push((work, plans));
    }
    for (work, plans) in &observations {
        assert!(work.selected, "{work:?}");
        assert_eq!(work.queries.len(), 2);
        assert!(work.steps > 0 && work.steps < 1500, "{work:?}");
        assert!(work.probe_steps > 0 && work.probe_steps < 250, "{work:?}");
        assert!(work.scans < 20, "{work:?}");
        assert!(
            plans
                .iter()
                .any(|p| p.contains("SEARCH g USING INDEX sqlite_autoindex_groups_1 (id=?)")),
            "{plans:?}"
        );
        assert!(
            plans.iter().any(|p| p.contains(
                "SEARCH a USING COVERING INDEX answer_statistics (group_id=? AND request_id=?)"
            )),
            "{plans:?}"
        );
        assert!(
            plans
                .iter()
                .any(|p| p.contains("SEARCH selected USING PRIMARY KEY (group_id=?)")),
            "{plans:?}"
        );
    }
    assert_eq!(observations[0].0.steps, observations[1].0.steps);
    assert_eq!(observations[0].0.probe_steps, observations[1].0.probe_steps);
}

#[test]
fn family_selection_preserves_distinct_parents_order_bins_filters_and_pages() {
    let mut selections = family_semantics(false);
    selections.extend(family_semantics(true));
    // Collect all complete-response evidence before checking path selection,
    // so the deliberately disabled fast path remains a semantic oracle.
    for (name, selected, expected) in selections {
        assert_eq!(selected, expected, "{name}");
    }
}

fn family_semantics(shared_groups: bool) -> Vec<(String, bool, bool)> {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("family-semantics.sqlite"), 7, 1000).unwrap();
    let mut records = Vec::new();
    for parent in 0..3 {
        let mut answers = Vec::new();
        for family in 0..if shared_groups { 110 } else { 140 } {
            let mut a = answer(
                &format!("g-{family:03}-{}", if shared_groups { 0 } else { parent }),
                &format!("q-{family:03}"),
                Some(&format!("family-{family:03}")),
            );
            match family % 3 {
                0 => {
                    a["value"] = json!([0.1, 0.9, 0.5][parent]);
                }
                1 => {
                    a["kind"] = json!("choice");
                    a["value"] = json!(["10", "2", "α"][parent]);
                    a["definition"] =
                        json!({"type":"choice","criteria":{"10":"Ten","2":"Two","α":"Alpha"}});
                }
                _ => {
                    a["kind"] = json!("score");
                    a["value"] = json!([1, 2, 3][parent]);
                    a["definition"] = json!({"type":"score"});
                }
            }
            if family == 9 && parent == 1 {
                a["valid"] = json!(false);
            }
            answers.push(a);
        }
        // Duplicate pair entries must not discard separate answers. This
        // family's legacy numeric values also expose changed SUM/AVG order.
        for (index, value) in [1e16, 1.0, -1e16].into_iter().enumerate() {
            let mut a = answer(
                &format!("order-{index}"),
                &format!("order-{index}"),
                Some("family-order"),
            );
            a["value"] = json!(value);
            a["confidence"] = json!(value);
            answers.push(a.clone());
            a["key"] = json!(format!("order-copy-{index}"));
            answers.push(a);
        }
        answers.push(answer("strict", "strict", None));
        let mut r = record(&format!("selected-{parent}"), "needle", answers);
        r["timestamp"] = json!(4_000_000_000_000_i64 - parent as i64);
        if parent == 1 {
            r["status"] = json!(500);
        }
        records.push(r);
    }
    records.push(record(
        "only-strict",
        "only-strict",
        vec![answer("strict", "strict", None)],
    ));
    records.push(record(
        "nul\0parent",
        "legacy-text",
        vec![
            answer("", "empty-id", Some("legacy-family")),
            answer("group\0suffix", "nul-id", Some("legacy-family")),
        ],
    ));
    records.extend((0..32).map(|index| {
        record(
            &format!("noise-{index}"),
            "noise",
            vec![answer("noise", "noise", Some("noise-family"))],
        )
    }));
    store
        .write_batch(&mut store.writer_connection().unwrap(), &records)
        .unwrap();
    let cases = [
        ("all", filter()),
        (
            "only-strict",
            Filter {
                model: Some("only-strict".into()),
                ..filter()
            },
        ),
        (
            "legacy-text",
            Filter {
                model: Some("legacy-text".into()),
                ..filter()
            },
        ),
        (
            "source",
            Filter {
                source: Some("source".into()),
                ..filter()
            },
        ),
        (
            "error",
            Filter {
                status: Some("error".into()),
                ..filter()
            },
        ),
        (
            "parent-search",
            Filter {
                search: Some("selected-1".into()),
                ..filter()
            },
        ),
        (
            "group-search",
            Filter {
                group_search: Some("FAMILY-00".into()),
                ..filter()
            },
        ),
        (
            "group-intersection",
            Filter {
                group: Some("family-000".into()),
                ..filter()
            },
        ),
        (
            "time",
            Filter {
                from: Some(3_999_999_999_999),
                to: Some(4_000_000_000_000),
                ..filter()
            },
        ),
        (
            "empty",
            Filter {
                source: Some("missing".into()),
                ..filter()
            },
        ),
        (
            "order",
            Filter {
                group_search: Some("family-order".into()),
                ..filter()
            },
        ),
    ];
    let mut selections = Vec::new();
    for (name, scope) in cases {
        let (result, work, _) = measured(&store, &scope);
        let prefix = if shared_groups { "shared-" } else { "" };
        evidence(&format!("{prefix}{name}"), &result, &work);
        if name == "all" {
            assert_eq!(result["summary"]["request_count"], 3);
            assert_eq!(result["groups"].as_array().unwrap().len(), 100);
            let next = Filter {
                group_cursor: Some(text(&result, "group_next_cursor")),
                ..scope.clone()
            };
            let (next, next_work, _) = measured(&store, &next);
            evidence(&format!("{prefix}next-page"), &next, &next_work);
            let first: std::collections::HashSet<_> = result["groups"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| text(g, "id"))
                .collect();
            assert_eq!(
                next["groups"].as_array().unwrap().len(),
                if shared_groups { 12 } else { 42 }
            );
            selections.push((
                format!("{prefix}next-page"),
                next_work.selected,
                shared_groups,
            ));
            assert!(next["group_next_cursor"].is_null());
            assert!(
                next["groups"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|g| !first.contains(&text(g, "id")))
            );
        } else if name == "only-strict" {
            assert_eq!(result["groups"].as_array().unwrap().len(), 1);
            assert_eq!(result["groups"][0]["is_family"], false);
        } else if name == "legacy-text" {
            assert_eq!(result["groups"].as_array().unwrap().len(), 1);
            assert_eq!(result["groups"][0]["answer_count"], 2);
            assert_eq!(result["groups"][0]["request_count"], 1);
        } else if name == "order" {
            assert_eq!(result["groups"].as_array().unwrap().len(), 1);
            assert_eq!(result["groups"][0]["answer_count"], 18);
            assert_eq!(result["groups"][0]["request_count"], 3);
        } else if name == "empty" {
            assert_eq!(result["summary"]["request_count"], 0);
            assert_eq!(result["groups"], json!([]));
        }
        let expected =
            name != "empty" && (shared_groups || matches!(name, "only-strict" | "legacy-text"));
        selections.push((format!("{prefix}{name}"), work.selected, expected));
    }
    selections
}

fn selected_parents(ids: impl IntoIterator<Item = String>) -> HashMap<String, i64> {
    ids.into_iter().map(|id| (id, 4_000_000_000_000)).collect()
}
fn selection(store: &Store, parents: &HashMap<String, i64>) -> &'static str {
    let reader = store.reader().unwrap();
    let result = Store::family_dashboard_selection(&reader, parents).unwrap();
    if result.is_empty() {
        assert_eq!(
            reader
                .query_row(
                    "SELECT count(*) FROM temp.sqlite_schema WHERE name='dashboard_family_pairs'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
    result
}

#[test]
fn family_selection_keeps_dense_oversized_and_legacy_fallbacks() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("family-fallbacks.sqlite"), 7, 5000).unwrap();
    let mut writer = store.writer_connection().unwrap();
    let records: Vec<_> = (0..2200)
        .map(|index| {
            record(
                &format!("r-{index}"),
                "needle",
                vec![answer("shared", "q", Some("family"))],
            )
        })
        .collect();
    store.write_batch(&mut writer, &records).unwrap();
    assert_eq!(
        selection(
            &store,
            &selected_parents((0..513).map(|i| format!("r-{i}")))
        ),
        ""
    );
    assert_eq!(
        selection(
            &store,
            &selected_parents((0..512).map(|i| format!("r-{i}")))
        ),
        FAMILY_PAIR_SELECTION
    );
    assert_eq!(
        selection(&store, &selected_parents(["r-0".into()])),
        FAMILY_PAIR_SELECTION
    );
    let dense = Store::open(&dir.path().join("dense.sqlite"), 7, 100).unwrap();
    dense
        .write_batch(&mut dense.writer_connection().unwrap(), &records[..4])
        .unwrap();
    assert_eq!(selection(&dense, &selected_parents(["r-0".into()])), "");
    for count in [128, 129] {
        let id = format!("groups-{count}");
        let answers = (0..count)
            .map(|index| {
                answer(
                    &format!("bounded-group-{index}"),
                    &format!("q-{index}"),
                    Some(&format!("bounded-family-{index}")),
                )
            })
            .collect();
        store
            .write_batch(&mut writer, &[record(&id, &id, answers)])
            .unwrap();
        assert_eq!(
            selection(&store, &selected_parents([id])),
            if count == 128 {
                FAMILY_PAIR_SELECTION
            } else {
                ""
            }
        );
    }
    let boundary: Vec<_> = (0..4096)
        .map(|index| answer("shared", &format!("k-{index}"), Some("family")))
        .collect();
    store
        .write_batch(&mut writer, &[record("boundary", "boundary", boundary)])
        .unwrap();
    assert_eq!(
        selection(&store, &selected_parents(["boundary".into()])),
        FAMILY_PAIR_SELECTION
    );
    let huge: Vec<_> = (0..4097)
        .map(|index| answer("shared", &format!("k-{index}"), Some("family")))
        .collect();
    store
        .write_batch(&mut writer, &[record("huge", "huge", huge)])
        .unwrap();
    assert_eq!(selection(&store, &selected_parents(["huge".into()])), "");
    let exact = "y".repeat(1024 * 1024 - "exact".len());
    store
        .write_batch(
            &mut writer,
            &[record(
                "exact",
                "exact",
                vec![answer(&exact, "exact", Some("exact-family"))],
            )],
        )
        .unwrap();
    assert_eq!(
        selection(&store, &selected_parents(["exact".into()])),
        FAMILY_PAIR_SELECTION
    );
    let long = "x".repeat(1024 * 1024);
    store
        .write_batch(
            &mut writer,
            &[record(
                "long",
                "long",
                vec![answer(&long, "long", Some("long-family"))],
            )],
        )
        .unwrap();
    assert_eq!(selection(&store, &selected_parents(["long".into()])), "");
    // A BLOB group identifier can still join in the original family SQL. Do
    // not coerce it into a TEXT temporary key and silently lose that answer.
    writer.execute_batch("PRAGMA foreign_keys=OFF; UPDATE groups SET id=CAST(id AS BLOB) WHERE id='shared'; UPDATE answers SET group_id=CAST(group_id AS BLOB) WHERE group_id='shared'; PRAGMA foreign_keys=ON;").unwrap();
    assert_eq!(selection(&store, &selected_parents(["r-0".into()])), "");
    for (name, scope) in [
        (
            "dense-fallback",
            Filter {
                model: None,
                ..filter()
            },
        ),
        (
            "fanout-fallback",
            Filter {
                model: Some("huge".into()),
                ..filter()
            },
        ),
        (
            "text-fallback",
            Filter {
                model: Some("long".into()),
                ..filter()
            },
        ),
        (
            "blob-fallback",
            Filter {
                search: Some("r-0".into()),
                ..filter()
            },
        ),
    ] {
        let (result, work, _) = measured(&store, &scope);
        evidence(name, &result, &work);
        assert!(!work.selected);
        assert!(!result["groups"].as_array().unwrap().is_empty());
    }
}
