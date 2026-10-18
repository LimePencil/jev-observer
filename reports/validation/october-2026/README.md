# October 2026 service improvement validation

Report prepared October 18, 2026, at 00:12 UTC. The session began October 17 at 17:24:20 UTC. Ten hours were requested, but the user asked to wrap up around 00:06 UTC, superseding that schedule. Active experiments were stopped and their partial outcomes preserved. **This is not a ten-hour completion claim or a release/deployment announcement.**

The frozen product source is `9bb84db756c76e834d6592d55dda5e9cd9689f3f`. The [issue and PR index](issue-pr-index.md) records **58 reviewed PRs merged**, including five already open at the start and 53 opened during the session. Report-only publication work is separate. Issues [#62](https://github.com/LimePencil/jev-observer/issues/62) and [#102](https://github.com/LimePencil/jev-observer/issues/102) remain open.

## Scope and evidence

Changes address capture allocation, transport accounting, import privacy/resource limits, encrypted history queries and maintenance, UI navigation/reviews, dependencies and CI cache reuse. All fixtures were synthetic. Service-level probes used loopback providers and disposable encrypted workspaces; standalone model and browser probes used bounded synthetic fixtures. No paid inference or production history/credentials were used.

Local timings and sampled RSS came from a shared Linux x64 host and describe those fixtures, not capacity guarantees. Rust allocation measurements exclude native memory. Exact-output comparisons, SQL VM counts and failing-with-fix-removed regressions support narrower implementation claims. Native verification below ran separately on six platform runners.

This archive contains selected summaries, the PR index and a [hash/schema manifest](evidence-manifest.json), with private paths and credentials excluded. Original hashes identify retained private evidence; raw databases, keys, giant exports and most session-specific harnesses are not bundled. Historical failures remain failed. The verifier checks archive integrity and allowed fields, not independent attestation or replay of every experiment.

## Executable provenance

The final local GNU/Linux executable is built from `9bb84db756c76e834d6592d55dda5e9cd9689f3f`, SHA-256 `26fac2283a9d2c5ce5d0971a77092dc3652578d18be57aef103bd7ad12dc7e08`. [Binary provenance](binaries.json) records full commits, trees, executable hashes and embedded assets for each distinct build. Native package identities are separate.

Earlier results on `ddc1804`, `61ae6c8`, `d66a9ae`, `da68897`, `0e04b1b`, `d55808a` and `69c7305` apply only to their named executables, not later code. The `0e04b1b` tree equals merged `db24268f0e27ac34114c08040823452eea23c17c`; its soak executable was never replaced during the run.

## Completed verification

- **Final integrated source:** `9bb84db` passed 190 Rust tests, with two intentional ignores, plus formatting, all-target Clippy and release build. Embedded assets match the production-preview UI suite: **129 UI tests—112 browser, eight API unit and nine server-rendering tests**.
- **Compatibility:** pinned Python SDK 0.7.1 and JavaScript SDK 0.6.0 produced 16 requests/48 answer records, including four expected errors, with wire/extension preservation and credential-exclusion checks. Published-0.1.0 upgrade/rollback/backup verification passed 36 checks; two packaged-backend browser journeys passed. Native credential-store persistence was not exercised.
- **Six native platforms:** [run 39080475558](https://github.com/LimePencil/jev-observer/actions/runs/39080475558) completed successfully on exact `9bb84db`: Linux x64/ARM64, macOS Intel/ARM64 and Windows x64/ARM64. Each passed 129 UI checks, two packaged-browser checks, SDK and upgrade checks. Rust counts were 190 on Unix and 183 on Windows, with two ignores per target. Aggregate archive verification succeeded; **publication was skipped**. Six validation ZIPs were downloaded and hashed. Executable/aggregate archives were checked through GitHub artifact metadata and successful verification steps, not separately downloaded for this review.
- **Transport/startup:** on `61ae6c8`, five injected transport failures plus five recoveries yielded ten persisted records/five incomplete captures, with permit recovery and sanitized errors. Direct/symlink FIFO inputs failed promptly without permission changes. Stress results distinguish intentional budget/writer-lock losses from unexpected gaps.
- **Imports/deep history:** normalization-budget rejection returns 413; malformed input remains 400; rejected batches leave no partial rows and release the slot. Encrypted HTTP checks cover accepted long metadata, dedup/identity, receipt privacy and stored-history recovery. Normalized JSON allows at most **144 nested object/array containers, including the root**; raw parser limits are unchanged.
- **Dense capture:** three alternating `0e04b1b`/`d55808a` pairs each preserved nine records/80,001 answers, exact wire bytes, ordered full-record projections and drained permits, with no unexpected gaps or storage failures. Projections exclude only generated root `id`, `timestamp` and `duration_ms`. Median sampled peak RSS was 193,768→172,760 KiB overall and 141,916→113,096 KiB during ingestion/drain. This measures the combined two array-ownership fixes, not their individual effects. A queued SIGTERM case exited zero in 2.970 seconds and reopened with all records/order intact.
- **Query work:** catalog seeks in [#114](https://github.com/LimePencil/jev-observer/issues/114) reduced 1,400,067→159 VM steps with nine complete-JSON comparisons on 100,000 parents. A separate million-parent diagnostic also measured 159 catalog VM steps, without a before/after timing claim. Explicit-date fixes in [PR120](https://github.com/LimePencil/jev-observer/pull/120) and [PR123](https://github.com/LimePencil/jev-observer/pull/123) retain exact output; the latter matched 60 complete JSON results and left no-date/one-bound query work unchanged.
- **Dependencies/CI:** compatible `rustls`/`source-map-js` patches and regenerated notices passed checks. Stable cache keys preserve build/test steps and only save successful main runs. Duplicate archive storage was observed; it does not establish an eviction cause or a build-speed gain.

## Endurance outcomes

**Completed passes:** the earlier `ddc1804` baseline offered 75,000 requests over five cycles with exact 1,000-record retention and two verified encrypted restarts. The `61ae6c8` browser segment completed 9,000.57 seconds and 76 drawer/filter workflows across four generations, with no steady request/page/workflow failures. Its informational console-classification limitation remains recorded; this does not prove absence of all memory leaks.

**Stopped at the user's request:** all shortened runs below remain partial, not full-duration passes. Owned service/browser/monitor processes were confirmed stopped; partial reports and original failure classifications were preserved.

| Run | Verified coverage before interruption | Boundary |
|---|---|---|
| Varied `61ae6c8` | 30/44 cycles; 900,000 fully checked offered requests; five verified restarts | 923,084 total offered includes an unfinished cycle |
| Varied `d66a9ae` | 22/36 cycles; 660,000 fully checked offered requests; three verified restarts | 663,421 total offered includes an unfinished cycle |
| Mixed `0e04b1b` backend | 179/210 cycles; 3,628 fully cycle-checked captures; five verified restarts | 3,648 forwarded includes 20 captures in the interrupted cycle; raw result remains failed |
| Mixed `0e04b1b` browser | 6,517.23/7,200 seconds; 55 full workflows, 29 dense/26 normal, four generations and 22 GC samples | `complete=false`; three body-observation failures remain in the gate |

Each completed varied cycle verified exact 1,000-record retention and zero dropped/truncated/write/maintenance failure counters. Requests in unfinished cycles are not claimed fully checked. The mixed workload uses 16×1,000-answer and four ordinary captures per cycle. Its last verified cycle retained 64 records/48,016 answers with exact ordered identity/privacy/projection checks. Warm-up counts are 16,048 then 32,032 answers, reaching 48,016 from cycle three. The template oracle is frozen production normalization, not independent semantic proof. The raw interrupted report records a bounded-fixture `TimeoutError`; its supervisor recorded no hard timeout or forced cleanup. This shortened run does not establish a new service defect or a full-run pass.

The mixed browser's three CDP response-body observation errors occurred near cancellations, but exact request identity was not recorded, so they cannot be retrospectively reclassified. Its source and errors are unchanged. Final polls were fresh; shutdown required no forced or leftover descendant cleanup. Maximum sampled app heap was 9.21 MiB and browser RSS 627.5 MiB. A separate read-only health/RSS/fd monitor collected 1,273 observations across two segments with zero anomalies and ended when the backend stopped; its complete flag describes observation coverage, not soak completion.

**Completed final-source pilots:** `9bb84db` passed two backend cycles in 93.094 seconds overall: 88 captures, 64 retained records, 16,048 then 32,032 retained answers, exact projection continuity across a 0.419-second encrypted restart, and clean fixture cleanup. The overlapping 60.45-second browser pilot covered both densities across two generations, completed two full workflows and ended with fresh polls and zero steady failures. Expected restart/cancellation events remain recorded. Its corrected observer classified a reproduced body error only with the exact same request's cancellation identity; it does not alter the older segment's failures.

**Not run:** the planned final-source 100-cycle backend and 90-minute browser follow-up were never launched after the user requested wrap-up.

## Preserved failures and open work

[Issue #102](https://github.com/LimePencil/jev-observer/issues/102) remains open. The first million-parent/three-million-answer HTTP probe on `0e04b1b` passed 26 data checks, but broad search took 27.10/24.94 seconds and the experiment failed overall: its private harness allowed only three seconds for final shutdown and forced termination. Without retained application logs, that does not establish a service shutdown defect. The original result is unchanged. The first Store PROFILE diagnostic also failed closed on a private guard's exit-transition `/proc` sampling race; cleanup succeeded. A separately reviewed retry passed nine full-JSON pairs and 20 helper exits, but median search remained 19.54 seconds. Unchanged unfiltered VM work does not support attributing its elapsed-time difference to code.

A separate million-parent counter diagnostic passed six complete-JSON comparisons; search took 21.66–25.10 seconds. Its overlapping CPU/pager/read observations do not establish physical-I/O causality or a special temporary-table slowdown.

The corrected actual-HTTP run on `69c7305` passed all 26 checks and clean shutdown: aggregates, foreign keys, timelines, pagination and four selected complete records—not every seeded body. The first unfiltered response took 22.942 seconds, its repeat 6.551 seconds and searches 13.316/9.263 seconds. All completed within that client's 30-second bound; the first valid response still exceeded the then-current 15-second UI deadline. Cross-run cache/resource conditions prevent a causal speed comparison.

[PR124](https://github.com/LimePencil/jev-observer/pull/124) therefore gives only dashboard reads a bounded 45-second deadline and a five-second slow-query notice. Health/details retain 15 seconds; writes are unchanged. This accommodation does not close #102. [PR121](https://github.com/LimePencil/jev-observer/pull/121) preserves pending review ownership across drawer close/reopen and stale GETs; its prior 124-test UI suite and focused production/StrictMode checks passed. All these fixes are included in final `9bb84db` verification.

[Issue #62](https://github.com/LimePencil/jev-observer/issues/62) tracks informational maintenance advisories for standalone `zvariant`/`zbus_names`/`zvariant_derive` in the Linux keyring chain, not a reported concrete vulnerability. A compatible lockfile-only zbus 6 migration was unavailable during the check.

Earlier harness failures, coverage gaps and corrected reruns are kept separate. Historical large live captures can exceed newer import acceptance limits; raw capture budgets are not total normalized-heap bounds.

## Fresh verification

From a clean checkout/output directory, build the UI before the executable and record new hashes. These commands do not reproduce historical binaries or every private experiment.

```sh
npm ci --prefix ui
npm run build --prefix ui
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked --example upgrade_probe
cargo build --release --locked
sha256sum target/release/jev-observer
(cd ui && npx playwright install chromium)
OBSERVER_UI_PREVIEW=1 npm test --prefix ui
OBSERVER_BINARY="$PWD/target/release/jev-observer" npm run test:integration --prefix ui
python3 scripts/check-retention-soak.py \
  --binary target/release/jev-observer --cycles 2 --seconds 3 --rate 50 \
  --max-records 120 --restart-every 1 --output retention-rerun.json
python3 reports/validation/october-2026/verify.py
```

Pinned SDK setup and upgrade-baseline checksum instructions remain in the repository's compatibility/CI documentation. The archive verifier requires no private input files and makes no release, deployment or service-capacity claim.
