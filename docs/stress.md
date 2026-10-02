# Local overload and recovery checks

## Encrypted history checks for 0.2.0

The October 2, 2026 SQLCipher checks passed capture-slot exhaustion, writer-lock recovery and a corrected burst run on a shared Linux ARM64 host with two Neoverse-N1 logical CPUs. These used candidate binary SHA-256 `e81762daa75859bc7d36d3c332e8a1c2d2740f8c26f1b1f358a36bf21cf24c7a`, before the final dashboard query optimization. They used the same small 402-byte request and 415-byte response as the older checks; no provider inference or real credential was involved.

| Pressure phase | Successful client calls | Persisted / visibly dropped | Client p95 / p99 | Recovery |
|---|---:|---:|---:|---|
| Four capture slots; 500/sec for 3 seconds; 100 ms mock | 1,500 | 92 / 1,408 | 140.84 / 163.56 ms | 20/20 persisted, no new loss |
| Locked writer; 500/sec for 3 seconds; 5 ms mock | 1,500 | 26 / 1,474 | 12.31 / 24.81 ms | 20/20 persisted, no new loss |
| Default limits; 500/sec for 5 seconds; 5 ms mock | 2,500 | 2,500 / 0 | 10.65 / 47.87 ms | 20/20 persisted, no new loss |

The [first encrypted report](../reports/stress/v0.2.0-encrypted.json) contains the passing capture-slot and writer-lock cases. The lock case reported 1,463 captures rejected before queueing and 11 accepted records lost during the injected write failure; these add to the 1,474 visible drops. Calls completed while the lock was held. All three accepted cases checked exact responses, usage totals, health access, encrypted database headers and complete recovery.

That first report is **failed overall** because its burst control phase exposed a harness bug: direct-to-mock requests carried an Observer access header that the mock correctly rejected. The proxy burst still retained all 2,500 requests. The corrected harness omits Observer metadata from direct control requests; the [separate burst rerun](../reports/stress/v0.2.0-encrypted-burst.json) passed every check. The original evidence remains unchanged.

The corrected burst had a 15.49 ms generator scheduling-lateness p99, 181 ms dashboard-query p95 and 39.1 MiB peak sampled proxy RSS. Its direct-to-mock control had a 255.54 ms client p99, showing considerable host variability even without Observer. These observations establish the recorded recovery behavior, not an isolated throughput limit or latency promise. The final dashboard optimization is measured separately in [performance measurements](performance.md).

To run the encrypted cases with a fresh report:

```bash
node scripts/stress.mjs --binary=target/release/jev-observer \
  --burst-rate=500 --burst-seconds=5 --output=reports/stress/encrypted-local.json
```

## Historical results before encrypted history

All three reference cases passed on September 22, 2026 using binary SHA-256 `eadc2bf4eca4f8edec0710de6a0a10394dd1f4999bd0c7d72049eee7d7eec71f` on the shared Intel N100 Linux host. The [complete reference report](../reports/stress/latest.json) records the executable, harness, runtime and effective settings. These are failure-injection checks on a 402-byte request and 415-byte response containing three questions, not a universal throughput guarantee.

| Reference pressure phase | Successful client calls | Persisted / visibly dropped | Client p95 / p99 | Result |
|---|---:|---:|---:|---|
| Four capture slots; 500/sec for 3 seconds; 100 ms mock | 1,500 | 96 / 1,404 | 105.85 / 116.15 ms | Expected loss visible; recovery passed |
| Locked writer; 500/sec for 3 seconds; 5 ms mock | 1,500 | 25 / 1,475 | 12.06 / 32.55 ms | Calls completed while locked; recovery passed |
| Default limits; 1,000/sec for 30 seconds; 5 ms mock | 30,000 | 30,000 / 0 | 27.85 / 131.35 ms | No missing captures; recovery passed |

The locked-writer run exposed one write failure: ten accepted records were lost when persistence timed out, and 1,465 captures were rejected before entering the queue. These sum to the 1,475 reported drops. Every case then persisted all twenty recovery calls, with no new losses. All client responses matched the fixture exactly, health and dashboard reads remained available, and all processes stopped cleanly. Peak sampled proxy RSS across the reference cases was 93.1 MiB; the burst's dashboard query p95 was 536 ms and its maximum sampled pending-record age was 315 ms.

An [initial 2,000/sec attempt](../reports/stress/initial-2000-failed.json) failed when the generator reached its 2,048-call concurrency bound; the mock had seen 29,123 calls. That earlier harness did not save complete partial-phase accounting. It remains a failed result. After improving diagnostics, a [separate 2,000/sec rerun](../reports/stress/burst-2000-diagnostic.json) retained all 60,000 calls with client p99 36.21 ms and full recovery. Its direct-to-mock baseline also exhibited a 934.97 ms p99 without Observer in the path. The variability does not establish a stable 2,000/sec limit or identify the cause of the earlier backlog; both outcomes remain in the evidence.

The [stress harness](../scripts/stress.mjs) injects recording failures while checking the complete client response, visible loss accounting and recovery. It uses Node.js builtins, Python for runtime telemetry, and a small Rust SQLCipher lock helper for the locked-writer case; it makes no provider calls and uses only a dummy credential. The harness generates a temporary database key. Build the executable using the [README](../README.md) first. Run stress cases separately from performance benchmarks or builds so contention is intentional and identifiable. The reference results above predate encrypted live history and should be rerun before using them as current capacity estimates.

Reproduce the complete reference suite:

```bash
node scripts/stress.mjs --burst-rate=1000 --burst-seconds=30 \
  --output=reports/stress/latest.json
```

The following table describes the harness defaults; the reference command above overrides the burst to 1,000/sec for thirty seconds.

| Case | Pressure | Required result |
|---|---|---|
| Capture budget | Four capture slots, four queue entries; 500 requests/sec for three seconds; 100 ms mock | Calls succeed unchanged, excess captures are visibly dropped, and later calls are recorded again |
| Locked writer | A Rust SQLCipher helper holds `BEGIN IMMEDIATE`; 64 slots/eight queue entries; 500 requests/sec for three seconds; 5 ms mock | Calls finish while the lock is still held; writer failure and drops are visible; recording recovers after unlock |
| Burst | Default capture limits; 2,000 requests/sec for five seconds; 5 ms mock | Responses remain correct, every call is either persisted or counted as dropped, and later calls are recorded again |

Each case starts its own Observer process and fresh local database. A brief warm-up precedes pressure. The burst also runs a direct-to-mock baseline at the same offered rate for up to ten seconds; those baseline calls are excluded from proxy accounting. After pressure ends and accepted work drains, a low-rate recovery phase must persist every new call without further drops or write failures. The first two cases require intentional capture loss; the burst reports whatever loss occurs and does not interpret zero loss as a requirement.

The harness asserts exact response bytes, HTTP success, upstream attempt count, three valid answers per retained request and usage counted once per retained request. After the queue drains, `persisted + dropped == forwarded`. It reports separately the captures never accepted into the queue and captures accepted but lost before persistence. A lost observation cannot contribute fabricated answers or usage.

During the locked-writer case, the lock remains held until all pressure requests have completed and health has been inspected. Calls must already be completing before SQLite's two-second busy timeout. Independent health polling remains active every 100 ms; a separate dashboard reader requests another snapshot one second after each completed response. Client p99 below one second and load-generator p99 scheduling lateness at most 100 ms are fixture-specific stall checks, not a production latency guarantee.

The output records the executable and harness SHA-256, host/runtime, effective settings, pressure/recovery counts, client latency distributions, sampled queue/lag/RSS/CPU, load-generator CPU and event-loop delay, exact assertions and process shutdown. CPU percentages use one core as 100%. RSS covers only the proxy; it is `null` where Linux `/proc` sampling is unavailable. If the concurrency bound is reached, the harness stops offering new calls, completes already offered calls, saves partial-phase accounting and attempts recovery; the case still fails. A failed assertion produces a nonzero exit and saves diagnostic evidence. Temporary databases are removed after the run unless `--keep-db` is supplied. To retain an encrypted database, supply your own `JEV_OBSERVER_DB_KEY` in the environment so it can be reopened later; the harness does not write the key into its report.

Use `--case=capture-budget`, `--case=locked-writer` or `--case=burst` for one case. `--binary=PATH`, `--python=python3` and `--work-dir=PATH` select the executable, Python runtime and database filesystem. The locked-writer case builds the Rust helper using Cargo. Burst settings are bounded to 10,000 requests/sec, 30 seconds, 100,000 total calls and 2,048 concurrent calls:

```bash
node scripts/stress.mjs --case=burst --burst-rate=3000 --burst-seconds=10 \
  --output=reports/stress/burst-3000.json
```

These small-payload tests establish recording-failure behavior for the recorded build and host. They do not cover every payload, query cardinality, retained-state configuration, disk failure, provider transport or operating system. See [performance measurements](performance.md) for representative mixed-payload sustained-load evidence.
