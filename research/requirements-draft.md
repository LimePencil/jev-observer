# Observer requirements discussion

Updated September 22, 2026. This document preserves the requirements discussion and its rationale. A working local prototype now implements the core workflow; the [README](../README.md) describes its actual behavior and the [implementation contract](../docs/implementation-contract.md) describes its interfaces. Earlier proposals below are not evidence that every acceptance target has been met.

## Implemented defaults and current scope

The user settled three simultaneous dashboard capabilities, continued forwarding during recording pressure, and a target of hundreds of requests per second. The implementation uses the following conservative defaults; numerical performance targets still need workload-specific evidence.

| Area | Implemented behavior |
|---|---|
| Packaging | Rust service, bundled React UI and SQLite; no Node.js runtime after building |
| Network boundary | Loopback listener, native `POST /v1/systemone`, one configured upstream; no added retry layer |
| Input privacy | No raw-state persistence by default; explicit `--capture-state` opt-in and configurable field redaction |
| Retention | Seven days and a soft cap of one million records; maintenance every 30 seconds, no disk-byte quota |
| Collection budget | 1,024 capture slots spanning active calls, queue and persistence; 256 KiB captured per body; queue capacity 1,024 |
| Overload | Drop excess capture work and mark incomplete observations while forwarding continues; no promised zero-loss audit trail |
| Collection health | Independent process-local drop, incomplete-capture and write-failure counters, current lag and last-gap time; counters reset on restart |
| Dashboard | Shared filters, recurring groups and typed distributions, request/failure feed, latency/usage/cost, detail panels, definition comparisons, review labels, light/dark themes |
| Refresh | Sequential polling after a one-second interval; inspection/pause holds the visible view, hidden tabs suspend polling |
| Local files | Observer JSONL and reviewed JevRouter receipt import; JSONL/CSV export; deliberate deletion; isolated synthetic sample workspace |
| Compatibility | Pinned official Python and JavaScript SDK checks passed through the proxy to a loopback mock; no live provider inference was used |

Grouping keeps incompatible definitions and presentations separate. The explicit `jgrep-v1` adapter recognizes one source-reviewed indexed-question pattern; no universal semantic mapping is claimed. JevRouter receipts remain application actions and do not inflate request usage or cost.

Offline threshold previews, matched-dataset evaluations/replay, durable gap history across restarts and verified macOS/Windows packaging remain outside the implemented slice. Shutdown now has a 10-second HTTP grace and a separate 15-second process watchdog. In particular, the original [MVP proposal](mvp.md) includes an offline-threshold acceptance item that is not yet delivered. Definition comparison in the dashboard is descriptive, not a paired evaluation experiment. Reproducible SDK evidence and its limits are in [compatibility notes](../docs/compatibility.md).

## Confirmed direction

- Observer must combine recurring-question statistics, live request/failure inspection, and latency/usage/cost monitoring. These are simultaneous capabilities, not alternative product directions.
- If recording cannot keep up, calls must continue without waiting for recording to recover. Missing history must be visible. The user explicitly selected this behavior.
- The target workload is hundreds of requests per second. This applies to useful observation with the dashboard active, not merely forwarding with capture disabled. Exact test rate, duration, hardware and payload profile remain to be specified.
- Study existing proxy implementations before finalizing the forwarding and collection design. Findings are recorded in [proxy performance research](proxy-performance.md).
- Use [Taste Skill](https://www.tasteskill.dev/) for visual guidance. The user identified this exact source. Its core and minimalist UI companion are installed locally.
- The initial discussion preceded implementation; retain its open workload and evaluation questions rather than treating proposed limits as approved or achieved results.
- Existing project direction remains a free, local, MIT-licensed application, with local storage, no account or automatic uploads, offline sample data, native TypeSafe collection and file import.

## Connected-dashboard rationale

The implemented developer dashboard uses compact information, clear hierarchy and restrained motion. The original interaction rationale follows; the status table above distinguishes delivered behavior from the wider proposal.

One overview should show all three capabilities together:

| Area | Contents | Connection to other views |
|---|---|---|
| Shared filters | Source/project, model, time window; optional supplied environment | Apply a consistent scope to metrics, groups and requests |
| Summary strip | Request volume, failures, latency, tokens and cost basis | Selecting a metric reveals the contributing requests |
| Question statistics | Searchable recurring definitions, answer distributions, trends and definition changes | Selecting a group filters its requests and exposes its versions |
| Live activity | Recent requests and failures, latency, usage, cost and answer count | Selecting a request reveals its answers and question groups |
| Detail panel | Definition, answer, alternatives, parent request and supplied outcomes | Preserve the surrounding investigation and selected filters |
| Collection status | Capture delay, missing/truncated records and storage trouble | Mark affected time windows so incomplete history is apparent |

Proposed interaction requirements:

- Keep collection running while the user inspects a record. Pause visible row movement, retain selection, and offer a new-requests counter and resume control.
- Show a common data timestamp for related views. Clearly label historical windows and pending records.
- Distinguish answer counts from distinct request counts. Global usage/cost comes from the request ledger; costs associated with different question groups can overlap.
- Keep unavailable cost, confidence and outcomes visibly unknown. An API failure is not a negative Jev answer; a model choice is not an executed application action.
- Start with a useful summary and a selected group's chart. Offer searchable detail without rendering a chart for every discovered question.
- Provide keyboard navigation, visible focus, accessible chart alternatives, loading/empty/error states, and readable light/dark themes.
- Use bundled typography and assets. Refresh visible data in bounded batches; background browser tabs and slow viewers must not build an unlimited notification backlog.

Taste Skill's current core describes marketing pages as its scope and excludes dense dashboards. Its relevant hierarchy, typography, theme and interaction guidance was applied contextually, together with the bundle's minimalist visual guidance. Operational tables, chart semantics and the user's simultaneous monitoring requirement take precedence over marketing-page spacing, hero imagery and animation prescriptions. The current UI uses React, selective Radix primitives, Phosphor icons, locally bundled Geist typography, restrained color and light/dark themes; density and interaction refinements can follow user feedback.

## Proposed performance requirements

- Reuse upstream connections through a long-lived HTTP client; preserve request and response semantics.
- Keep storage, normalization, chart aggregation and browser updates outside the forwarding path wherever possible.
- Bound captured bytes, active captures, queued work and background tasks. A queue's entry count alone is not a memory limit.
- Batch persistence and UI notifications independently. Record capture gaps and persistence delay.
- Preserve one request's accounting across its linked answers. Avoid adding hidden retries, caching or additional inference calls to observation.
- Benchmark a deterministic upstream directly and through Observer, including capture enabled and active dashboard reads. Measure client latency distributions, throughput, memory, persistence lag and recording completeness together.
- Separate warm connections, cold connections, large batches, bursts, slow storage, cancellation and process restart. Agree on numerical limits after choosing the target workload and hardware.

These design choices were informed by source inspection. Performance results must come from the application's own documented measurements; this rationale does not establish an overhead or throughput guarantee.

## Performance acceptance draft

Translate "hundreds of requests per second" into a provisional benchmark at **500 requests/second sustained**, with a separate **1,000 requests/second burst** scenario. These exact numbers are proposed test points, not user-approved limits or achieved results. Specify representative payload bytes, answers per request, upstream response delay, hardware and run duration before making the test an acceptance gate.

- Within the agreed supported workload, forwarding and collection must both keep up: no proxy-induced request failures, no rejected captures, and complete supported request/answer records after the queue drains. Deliberate retention choices are reported separately from dropped records. Permission to drop history during overload does not make routine capture loss at the target load acceptable.
- Test all three dashboard capabilities together. Collection lag must remain bounded, and historical queries and live refreshes must not create unbounded work.
- Under forced storage stalls, full queues or capture-budget exhaustion, stop accepting excess capture work without intentionally delaying forwarding. Keep memory bounded and expose gap counts, affected periods and recovery status independently of the failed storage writer.
- Keep capture limits separate from upstream request-body limits. Skipping or truncating observation must not truncate the forwarded body.
- Refresh the dashboard in batches rather than once per request. Proposed freshness target: recorded activity visible within one second during normal load, to be measured with the selected workload.
- Measure client completion latency and proxy processing stages, including tail latency, against the same deterministic upstream. Latency and memory budgets remain open; do not substitute vendor claims or differences between unrelated percentile values.
- Ctrl-C/SIGTERM cancels upstream work, allows at most 10 seconds of HTTP grace, then drains queued captures. A separate watchdog forces exit status 2 at 15 seconds total if shutdown or runtime cleanup remains blocked, with a warning that pending captures may be lost. Exceeding HTTP grace also returns nonzero after draining. Abrupt termination or forced deadlines can lose observations; durable gap history across restarts remains unimplemented. "Calls keep moving" applies to recording failures while the forwarding process is running.

## Remaining workload and product decisions

1. Which investigations need retained input state? The implemented default is off with explicit redacted capture available; exact replay and offline threshold previews are not implemented.
2. Are requests predominantly small checks, large inputs/batches, or a mixture? Specify typical/maximum batch and payload sizes and upstream latency for the hundreds-per-second target.
3. Are the seven-day/one-million-record defaults useful, and is a byte-based disk quota needed? What additional capture or gap history must survive abrupt termination?
4. Which operating systems/runtime should be verified first, and what exact load, duration, latency, memory, capture-freshness and UI-response targets define success there?
5. Does the working compact dashboard fit the user's preferred density and theme? Refine it from an actual investigation with representative local records.

The main workflow, failure behavior and conservative capture/retention defaults are implemented. All three dashboard capabilities, continued forwarding during recording pressure, and the hundreds-per-second target remain settled; supported workload limits and the deferred evaluation features need separate validation and scope decisions.

## Measured acceptance update

The final local runs verified 500 requests/sec for five minutes and a 1,000 requests/sec burst with complete recording, plus 500 requests/sec against a one-second mock upstream. See [performance measurements](../docs/performance.md) for exact fixtures and evidence. Whole-history dashboard query p95 reached 2.94 seconds around 150,000 records, so the proposed one-second display freshness is not achieved at that scale; forwarding and recording remain independent. The confirmed throughput and overload requirements are verified within the documented workload.
