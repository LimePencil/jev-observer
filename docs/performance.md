# Local performance measurements

Measured September 22, 2026 on an Intel N100 (4 logical CPUs), 7.0 GiB RAM, Linux x64 and an NVMe filesystem. The proxy, Node.js mock/load generator and browser shared the machine. These are local measurements, not provider capacity or an SLA. No paid inference or real credential was used.

**Both baseline load checks passed:** no client errors, dropped/incomplete captures or database write failures; every expected request and answer was retained, and token accounting remained once per request. These checks cover commit `c202350`, release binary SHA-256 `15433574df562d6b87b995008b04c3e0496326d5631efb543afb98559c8a65f7`. That executable, including the dashboard, is 10,040,720 bytes (9.6 MiB). The subsequent query optimization and stress checks are documented below, with their own executable hashes.

## Results

| Scenario | Offered duration | Steady completed requests/sec | Calls / answers retained | Client p50 / p95 / p99 | Dashboard query p95 |
|---|---:|---:|---:|---:|---:|
| 25 ms mock, sustained | 300 s | 500 | 150,000 / 705,000 | 26.03 / 27.91 / 31.15 ms | 2.94 s |
| Same process/history, burst | 10 s | 1000.2 | 10,000 / 47,000 | 25.93 / 26.96 / 27.78 ms | 2.48 s |
| 1,000 ms mock, sustained | 60 s | 500 | 30,000 / 141,000 | 1000.68 / 1001.68 / 1003.31 ms | 385 ms |

The five-minute run and burst peaked at **162.7 MiB proxy RSS**, with maximum **sampled** pending-record age 95 ms and queue depth 48. The one-second-upstream run peaked at **202.4 MiB**, with sampled pending age 33 ms. These are one-second samples of the proxy process, not total browser/generator memory or a hard upper bound.

**The baseline exposed a dashboard freshness limitation.** Exact whole-window summaries grew more expensive as history grew; the earlier one-second display-freshness proposal was not met at roughly 150,000 records on this machine. That version waited for the current snapshot and then paused one second. The [large-history refresh changes](#large-history-dashboard-refresh) below address both costs. A recorded 90-second real-browser segment received 24 snapshots while its request count grew from 99,575 to 142,721, with no JavaScript errors or page overflow. Forwarding and recording continued independently. The separate benchmark dashboard reader stayed active throughout every proxy phase.

Raw evidence: [sustained and burst](../reports/benchmarks/sustained.json), [slow upstream](../reports/benchmarks/slow-upstream.json), [real-browser segment](../reports/benchmarks/dashboard-browser.json). Both real client packages are also verified against the subsequent executable; the [compatibility evidence](compatibility.md) records its exact hash.

## Query optimization comparison

Strict question groups now calculate counts, means and distributions in one ordered pass over their answers. Distinct parent counts use the ordered request IDs; means use compensated summation to preserve SQLite's results. Only the latest 100 candidate groups are retained. Explicit families keep their exact SQL distinct counts.

The [paired comparison](../reports/benchmarks/dashboard-comparison.json) tested baseline `15433574…` against candidate `eadc2bf4eca4f8edec0710de6a0a10394dd1f4999bd0c7d72049eee7d7eec71f`, sequentially on separate backups of the same synthetic fixture. It contains 150,000 requests and 705,000 answers across twenty definitions: a 30,000-request mixed-payload capture was copied five times with distinct request IDs. This measures query work on a quiet database, separately from forwarding capacity under load.

| Filter | Baseline | Candidate | Observations per binary |
|---|---:|---:|---:|
| All history | 2,212 ms | 1,662 ms | Median of 5 after a warm-up |
| Last 24 hours | 2,374 ms | 1,540 ms | 1 |
| Last hour | 2,444 ms | 1,451 ms | 1 |
| Source | 2,377 ms | 1,485 ms | 1 |
| Model | 2,383 ms | 1,480 ms | 1 |
| Search | 9,878 ms | 4,694 ms | 1 |
| Failures only | 669 ms | 697 ms | 1 |

Every response matched exactly after excluding only the generated timestamp and process-local health. Counts, array order, distributions and numeric means were included. Source database/WAL/SHM hashes remained unchanged, and neither process forwarded a call. The all-history candidate samples ranged from 1,517 to 2,999 ms; these few observations establish neither a p95 nor a one-second freshness guarantee. Search and large-history refresh remain areas for further improvement.

Use a stopped, synthetic fixture and separately built baseline/candidate executables:

```bash
python3 scripts/compare-dashboard.py \
  --baseline /path/to/baseline-observer \
  --candidate target/release/jev-observer \
  --fixture /path/to/quiet-fixture.sqlite
```

For overload, injected writer failures and recovery on the candidate, see [stress testing](stress.md). Intentional capture losses and unsuccessful capacity probes are reported explicitly there.

## Reusing search selections

A further optimization evaluates search/group filters once per dashboard snapshot and reuses the matching parent sequence IDs across its queries. This avoids repeating correlated answer-key lookups for every answer and aggregate. The selection is connection-local, stores only integer IDs and uses a 2 MiB temporary pager-cache target. It does not modify the main database or cache results across snapshots. For more than 100 strict definitions, candidate ranking now precedes distribution calculation.

The [search comparison](../reports/benchmarks/search-comparison.json) compares `eadc2bf4…` with `ec5f0ae0f5ca6ef1f8267bb5dc4560747096736638a01c67a6bd5ec445d7c047` on the same 150,000-request / 705,000-answer fixture. Both filters used five observations per binary:

| Filter | Previous median | New median |
|---|---:|---:|
| All history | 1,248 ms | 1,257 ms |
| Search `urgency` | 3,908 ms | 1,780 ms |

Search was 2.2 times faster in this run; the unfiltered timing was essentially unchanged. All response hashes matched exactly after excluding only generation time and live process health. Source files were unchanged. These are quiet-database query measurements, not a p95 or a sub-second freshness claim.

```bash
python3 scripts/compare-dashboard.py \
  --baseline /path/to/previous-observer \
  --candidate target/release/jev-observer \
  --fixture /path/to/quiet-fixture.sqlite \
  --filters=all,search --samples-per-filter=5 \
  --output=reports/benchmarks/search-comparison.json
```

The comparison tool can select any of its seven existing filters. When `all` is omitted, a separate all-history count check still verifies that fixture records were preserved.

The same new binary also passed a [mixed-payload load check with search active](../reports/benchmarks/search-mixed-load.json): 500/sec for 60 seconds followed by 1,000/sec for 10 seconds. All 40,000 requests and 188,000 answers were retained, with zero client errors, dropped/incomplete captures or write failures. Client p95/p99 were 26.82/28.53 ms during the sustained phase and 27.98/31.06 ms during the burst, including the 25 ms mock upstream delay. Search query p95 was 426 ms as history grew to 30,000 requests and 627 ms during the burst to 40,000. Peak sampled proxy RSS was 74.5 MiB, pending-record age 65 ms and queue depth 48.

This run had substantial spare host capacity: median sampled whole-host CPU busy was about 25% in the sustained phase and 41% in the burst. It is separate from the failed contended-host run below and does not erase that outcome or establish a universal capacity guarantee.

```bash
node scripts/benchmark.mjs --seconds=60 --rate=500 --baseline-seconds=5 \
  --dashboard-search=urgency --output=reports/benchmarks/search-mixed-load.json
```

## Large-history dashboard refresh

The [latest paired comparison](../reports/benchmarks/large-history-comparison.json) uses the same **150,000-request / 705,000-answer / twenty-definition** fixture. Baseline `ec5f0ae0…` is the previous search-optimized executable; candidate `25245165f030b29d7d0ad5ccceadefcf9a9a0975244b707c8e66ffabf335c806` includes the new query path and dashboard branding. Each of the seven filters received five timed queries on separate database copies, with an additional all-history warm-up.

| Filter | Previous median | New median | Speedup |
|---|---:|---:|---:|
| All history | 1,240 ms | 574 ms | 2.16× |
| Last 24 hours | 1,423 ms | 593 ms | 2.40× |
| Source | 1,401 ms | 600 ms | 2.34× |
| Model | 1,410 ms | 600 ms | 2.35× |
| Search `urgency` | 1,742 ms | 894 ms | 1.95× |

All seven complete response comparisons matched exactly after excluding only generation time and process-local health, including totals, percentiles, means, distributions, feed contents and array ordering. Source database/WAL/SHM files remained unchanged. The last-hour and failures-only filters were empty in this fixture; their timings are recorded but are not evidence of populated-history performance. These five-sample medians describe a quiet database, not a p95 or a guarantee for every history size.

The query path now reads a compact covering index once for request totals, timeline buckets, exact nearest-rank latency percentiles and selected parent IDs. Strict question statistics look up those parents in a snapshot-local map instead of making a SQLite request-index lookup for every answer. Broad searches resolve answer-key matches in one scan, and their request feed walks newest-first until it has 100 matches. Full selected-history statistics remain exact; the feed and displayed groups retain their existing 100-item limits. Existing supported databases receive the index automatically on startup. The index consumes additional disk space and write work; the parent map and percentile buffer use memory proportional to the selected request count. No result cache survives across snapshots.

Successful browser polls now count query time toward a one-second start-to-start interval instead of adding a second after every response. They remain non-overlapping, with at least 100 ms idle time after a slow query and a one-second retry delay after failure. Hidden-tab cancellation, review pause and stale-response guards remain in place. Deterministic browser tests cover the timing and failure cases. Rust regressions compare against the former SQL aggregates and sorted percentiles, including fractional costs, more than 168 timeline buckets, timestamp-less imports, mixed actions, filters, retention and deletion.

```bash
python3 scripts/compare-dashboard.py \
  --baseline /path/to/previous-observer \
  --candidate target/release/jev-observer \
  --fixture /path/to/quiet-fixture.sqlite \
  --samples-per-filter=5 \
  --output=reports/benchmarks/large-history-comparison.json
```

The same executable passed a [five-minute live-history run](../reports/benchmarks/large-history-live-load.json) with search active, followed by a ten-second burst. All **160,000 requests and 752,000 answers** were retained, with zero client errors, dropped/incomplete captures or database write failures. The workload and 25 ms mock delay match the mixed-payload fixture described below.

| Live phase | Steady requests/sec | Client p95 / p99 | Search query p95 |
|---|---:|---:|---:|
| 500/sec for five minutes | 500 | 26.76 / 28.49 ms | 933 ms |
| 1,000/sec for ten seconds | 1,000 | 26.92 / 27.59 ms | 1,033 ms |

Peak sampled proxy RSS was **144.6 MiB**. A separate [real-browser observation](../reports/benchmarks/large-history-browser.json) ran alongside the search reader for 90 seconds, as history grew from **100,750 to 145,742 requests**. It received **91 snapshots**, with median/p95/maximum query times of **573/676/696 ms**, no browser errors and no horizontal overflow. The [captured dashboard](../reports/benchmarks/large-history-browser.png) also shows the shared website branding. The browser used the default last-24-hours overview; the benchmark's second reader used all-history search. Its one-second post-response delay remains a harness behavior, separate from the improved UI cadence.

This verifies roughly one overview refresh per second for the recorded live workload. The burst search reader exceeded one second, and these results do not establish million-record, arbitrary high-cardinality or large-family query performance. The new index and snapshot map do not provide a universal latency or memory bound.

```bash
node scripts/benchmark.mjs --seconds=300 --rate=500 --baseline-seconds=5 \
  --dashboard-search=urgency --output=reports/benchmarks/large-history-live-load.json
# In another terminal, after history reaches approximately 100,000 requests:
node scripts/watch-dashboard.mjs --origin=http://127.0.0.1:PORT --seconds=90 \
  --output=reports/benchmarks/large-history-browser.json
```

## Initial installable Linux release

The archived prelaunch static musl build (unrelated to the fresh public `v0.1.0`) has binary SHA-256 `83ae268b1ca2aa3a7de482337f0bef6fc77cb1191d3d604ffec9f2bc412bda58`. Its [separate load check](../reports/benchmarks/release-linux-x86_64.json) passed with search active: all 40,000 requests and 188,000 answers were retained, with zero client errors, dropped/incomplete captures or write failures.

| Phase | Steady requests/sec | Client p50 / p95 / p99 | Search query p95 |
|---|---:|---:|---:|
| 500/sec offered for 60 seconds | 499.98 | 25.90 / 28.14 / 31.56 ms | 635 ms |
| 1,000/sec offered for 10 seconds | 998.4 | 28.48 / 51.80 / 80.96 ms | 1,659 ms |

The mock adds 25 ms. Peak sampled proxy RSS was 57.9 MiB, pending-record age 563 ms and queue depth 574. Burst dashboard refresh exceeded one second. This is a separate build and run from the glibc executable above; other test processes shared the host, so differences do not isolate the effect of the C runtime. All 56 Rust tests also passed for the musl release target, and [both SDK fixtures passed against this exact executable](../reports/releases/prelaunch-linux-x86_64-sdk.json).

```bash
node scripts/benchmark.mjs \
  --binary=target/x86_64-unknown-linux-musl/release/jev-observer \
  --seconds=60 --rate=500 --baseline-seconds=5 --dashboard-search=urgency \
  --output=reports/benchmarks/release-linux-x86_64.json
```

## Failed mixed-payload run under host contention

A subsequent [mixed-payload run](../reports/benchmarks/contended-mixed-load.json) on candidate `eadc2bf4…` **failed** its forwarding, capture-completeness and offered-rate checks. The 500/sec phase and 1,000/sec burst together returned 996 unsuccessful or mismatched client responses out of 40,000 calls; 6,267 captures were dropped and 671 retained captures were incomplete. These are failed results, not acceptance evidence.

During that run, unrelated installs, builds and test processes were observed on the same machine. A `vmstat` sample showed roughly 47 runnable tasks, zero idle CPU and approximately 7 GiB of used swap on the 7 GiB host. The load generator also missed its offered schedule. This establishes substantial contention during the failed run, but does not isolate a single cause for every error or establish the proxy's capacity in isolation. The earlier successful small-payload stress cases do not replace this larger-payload check.

## Workload and checks

- Fixed offered rate, HTTP keepalive, identity-encoded native TypeSafe JSON. The mock's idle timeout exceeds the test duration; transport failures/cancellation are exercised separately in Rust tests.
- 90% 9,764-byte requests with three questions; 10% 131,744-byte requests with twenty questions. Every response contains mixed Choice, Score and Noul observations. The twenty definitions recur; this is not a high-cardinality or large-family benchmark.
- Default capture behavior: input state not persisted, 256 KiB captured per body, 1,024 capture slots and queue capacity 1,024. Slots span active calls, queued events and persistence. The benchmark raises the record cap to two million; neither that nor the one-million default is reached.
- Every successful response is compared byte-for-byte with its expected fixture. Status, request/answer counts, valid observations, input/output token totals and upstream attempt totals are checked. Cost remains unknown because no pricing was configured.
- SQLite 3.53.2 is bundled. WAL, batched transactions, a 32 MiB writer cache target and 8 MiB reader cache targets are enabled. Cache memory grows on demand. Native UUIDv7 IDs and covering indexes reduce write/query work.
- Client completion and first-byte distributions include the proxy and mock. Steady RPS excludes the stated five-second warm-up and post-load drain; `achieved_rps` in JSON includes the drain. Subtracting independent p95 values would not produce a per-request overhead percentile.

The long run retained about 0.98 GiB of SQLite data for 160,000 requests. Storage depends on definitions, answers and extension fields; this is not a universal bytes-per-request estimate. At a constant 500 requests/sec, the default one-million record cap holds about 33 minutes, even though the maximum age is seven days.

## Reproduce

Build first using the [README](../README.md), then run these sequentially:

```bash
node scripts/benchmark.mjs --seconds=300 --rate=500 --output=reports/benchmarks/sustained.json
node scripts/benchmark.mjs --seconds=60 --rate=500 --upstream-ms=1000 --no-burst --output=reports/benchmarks/slow-upstream.json
```

The harness creates temporary ports and a database on the project filesystem, then removes its database on completion. `--keep-db` retains it for inspection; `--work-dir=/path` selects a filesystem. `--baseline-seconds=N` overrides the positive duration of the direct mock control phase. `--dashboard-search=TEXT` exercises search during load; final accounting still checks the entire retained history. Reports identify hardware, binary hash, limits, capture health and counts. New runs also sample whole-host CPU, load average, available memory and swap each second to expose shared-machine contention; unsupported measurements remain null. A nonzero exit indicates a failed check.

To add a real browser, install the UI's Playwright Chromium and, in another terminal, use the `observer_origin` printed by the running benchmark:

```bash
node scripts/watch-dashboard.mjs --origin=http://127.0.0.1:PORT --seconds=90
```

Choose a browser duration that ends before the benchmark stops its proxy, or stop the companion with Ctrl-C while the proxy still runs so it can save its report. This adds a dashboard reader to the harness's existing polling. The original byte/status, streaming, overflow, SQLite-lock/recovery, cancellation, retention, export and shutdown cases are covered by `cargo test --locked`.

These runs do not establish cold TLS latency, real provider behavior, full input-state capture capacity, million-record dashboard latency, every filter/cardinality, or other operating systems. Compressed bodies are forwarded unchanged but saved as incomplete; the SDK setup requests identity encoding for typed capture. Earlier diagnostic reports in `reports/benchmarks` describe development configurations and are not final acceptance results.
