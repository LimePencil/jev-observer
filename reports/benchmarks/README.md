# Benchmark evidence

Each report records its own release executable hash. Baseline acceptance reports cover commit `c202350`:

- `sustained.json`: 500 requests/sec for five minutes, followed by a ten-second 1,000 requests/sec burst. All checks passed.
- `slow-upstream.json`: 500 requests/sec for one minute with a one-second mock upstream delay. All checks passed.
- `dashboard-browser.json` and `.png`: a successful 90-second browser segment during the sustained run.

`dashboard-comparison.json` compares that baseline with the subsequent query optimization on identical 150,000-request fixtures. All seven response comparisons passed. It reports quiet-database query timings, not forwarding capacity. See also the separate [overload and recovery evidence](../../docs/stress.md).

`contended-mixed-load.json` is a failed follow-up run on the candidate while the shared host was heavily loaded and swapping. Client errors, capture losses and missed offered rates remain in the report. It is not acceptance evidence.

`search-comparison.json` measures the next search optimization on the same 150,000-request fixture, repeating both all-history and search queries five times per binary. Exact response comparisons passed; source fixture files were unchanged.

`search-mixed-load.json` checks that same new binary at 500/sec for 60 seconds and 1,000/sec for 10 seconds, with search polling active. All 40,000 requests / 188,000 answers were retained and every check passed. Host telemetry records the capacity available during this run; this result does not replace the failed contended-host evidence.

See [performance measurements](../../docs/performance.md) for the workload, measured limits and reproduction commands.

The remaining files are development diagnostics, not final acceptance evidence. `before-write-tuning.json` and `slow-upstream-before.json` exposed capture losses that prompted subsequent tuning. `smoke.json` and `aggregate-queries.json` cover earlier binaries and shorter workloads. `browser-before-write-tuning.json` and `.png` record an earlier UI check. `initial.json` used an unauthorized mock fixture and does not measure successful forwarding performance.

`large-history-comparison.json` compares the subsequent compact-index and single-pass dashboard implementation with the previous search-optimized executable. All seven response comparisons matched exactly over five samples each; see [large-history dashboard refresh](../../docs/performance.md#large-history-dashboard-refresh) for populated-filter timings and empty-filter caveats.

`large-history-live-load.json` verifies the same executable at 500 requests/sec for five minutes and a 1,000/sec ten-second burst with search active. `large-history-browser.json` and `.png` record a concurrent 90-second overview observation above 100,000 retained requests. All capture/accounting checks passed; burst search p95 still exceeded one second.
