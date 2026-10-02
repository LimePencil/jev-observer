# Deep audit, September 22, 2026

This audit starts from `ecad07880e9b9be2060089fe2a8e5d2633716967`. It covers normalization/import, SQLite reads and exports, startup, proxy/collector behavior, browser state, and installation checks. Findings were checked against concrete inputs before fixing them; this is not a claim that every possible defect has been eliminated.

## Confirmed changes

| Problem | Result and regression coverage |
| --- | --- |
| Group details replaced an existing group filter, so their feed, timeline, observations, and version counts could disagree with the overview. | Preserve the caller's parent scope and intersect it with the requested detail group. Tests cover strict groups, families, overlapping parents, and an empty intersection. |
| An explicit empty group query selected nothing. | Treat `group=` like an omitted filter consistently in dashboards, details, and both exports. |
| An answer's type could stand in for a missing type in its question definition. | Preserve the raw answer but mark the observation invalid; test all three supported types and recomputation during import. |
| Native empty question keys and manually created empty review keys could be exported but not reimported. | Preserve every string key, including empty strings, while rejecting non-string keys. |
| Restarting a demo containing only imported actions added synthetic requests. | Check whether any event exists before seeding; test action-only history and repeated startup. This also removes a full dashboard aggregation from startup. |
| Source/model controls appeared cleared during a scoped request or failure even though their filters were still applied. | Preserve catalogs and selected values through loading, errors, recovery, and retention changes. Separate browser regressions cover each control. |
| One successful health request could hide newer dashboard gap counters when later health requests failed. | Use ordered server health samples across the dashboard and independent endpoint. Independent review also reproduced delayed-response reordering; regression coverage includes that case and process transitions. |
| CSV export reconstructed all answer definitions and reviews to obtain an answer count. | Read saved parent metrics and the transactional answer count. Coverage checks null values, multiline/quoted fields, formula escaping, invalid answers, actions, and concurrent deletion. |
| Numeric formatting created a new internationalization formatter for each cell. | Reuse formatters while retaining zero/unknown handling, compact notation, and all cost precision boundaries. |

The group, empty-filter, definition-type, empty-key, demo, filter-control, and health regressions were observed failing before their corresponding fixes. Initial browser failures were observed through tool output; those original traces were overwritten by subsequent successful runs. The later independent health-ordering review retains separate [three failing cases before sample ordering](out-of-order-before-fix.log) and [five passing health cases afterward](out-of-order-after-fix.log). Trace ZIPs are not included in this archive.

## Bounded performance observations

- **CSV query work:** five warmed debug-build samples on 200 requests × 10 answers × 8 KiB definitions had median times of **202.44 ms before / 18.23 ms after**. Both implementations produced the identical 22,516-byte CSV and SHA-256. [Fixture, samples, and hash](csv-benchmark.json). Run `cargo test --locked csv_export_large_definitions_benchmark -- --ignored --nocapture` to obtain new candidate samples. The benchmark is intentionally excluded from ordinary tests and has no timing pass/fail threshold.
- **Number formatting:** 30,000 formatting calls took **1,818 ms before / 47 ms after**, with identical outputs. A second run measured 1,931/54 ms. Run `node reports/validation/deep-audit/format-benchmark.cjs`; [original measurement](format-benchmark-original.json), [reproduction](format-benchmark-reproduction.json).

These are synthetic microbenchmarks on the shared development host, not whole-application speedups, service throughput, p95 estimates, or universal latency guarantees. The formatting script compares equivalent isolated expressions; ordinary formatting tests exercise the actual exported UI functions.

## Snapshot test quality

The CSV deletion regression now seeds two parents and deletes both after reading the first parent. It must return the complete original page. A single-parent test would no longer detect the missing transaction after the CSV optimization because that row's scalars have already been loaded.

The [fresh mutation manifest](snapshot-mutations.json) and adjacent test logs establish:

| Isolated source variant | Pass | Fail |
| --- | ---: | ---: |
| Current source | 5 | 0 |
| Request transaction removed | 3 | 2 |
| Export transaction removed | 2 | 3 |
| Both transactions removed | 0 | 5 |

The mutation script relaxes both shared helper signatures in isolated copies and never edits product sources. The source hashes identify the exact inputs. Its release hash is an unchanged-executable guard, not a claim that the release binary already includes these edits. The separate deterministic storage harness also reran all twelve baseline/current schedules successfully after updating its insertion marker. Recorded observations remain unchanged; public report paths use `<repo>` in place of the original checkout prefix.

Reproduction prerequisites and commands are in the [parent validation guide](../README.md).

## Local verification

- `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings` passed.
- `cargo test --locked`: **79 passed**, one explicitly ignored benchmark (run separately for the measurement above).
- `npm --prefix ui run build` passed; `npm --prefix ui test -- --workers=1`: **29 passed**, including browser interaction, accessibility, and formatting/health helpers.
- `python3 scripts/test-install.py`: **21 passed**.
- `cargo build --release --locked` passed. Real Python 0.7.1 and JavaScript 0.6.0 SDKs passed against a loopback mock: eight upstream attempts, 24 observations, no capture gaps. The HTTP check also verifies that dashboard and independent health responses share the same process identity and increasing sample order. [Release hash and results](sdk-compatibility.json).
- The historical evidence verifier, the updated deterministic storage harness, and all four public snapshot mutation variants passed their expected-result checks.

This adds **8 Rust tests and 10 UI tests**, plus the optional CSV benchmark, and strengthens the existing CSV concurrency regression.
