# Proxy performance research

Reviewed September 22, 2026. This is a source review and proposed validation plan, not measured Jev Observer performance. No third-party proxy or benchmark was executed. Links below reference the branch snapshots inspected on this date; they are not pinned commits and may change. Recommendations remain proposed while requirements are being discussed.

The user confirmed all three dashboard capabilities: recurring questions and trends; live requests and failures; latency, usage and cost. Their simultaneous use belongs in performance testing. The user subsequently selected continued forwarding when recording falls behind and a target of hundreds of requests per second. Exact traffic profile, payload sizes, machine budget and capture limits remain unresolved; see the [requirements discussion](requirements-draft.md).

## Findings and proposed choices

**Reuse connections before pursuing custom optimizations.** LiteLLM initializes a shared HTTP session with keepalive, DNS caching and total/per-host connection limits. Pingora automatically pools completed upstream connections, reusing them only when connection-relevant peer attributes match. Jev Observer should keep one reusable HTTP client and explicitly limit concurrent upstream work. This supports the proposed Rust service without establishing that Rust alone guarantees low overhead. Defer custom object pools and transport specialization until profiling identifies a problem. [LiteLLM startup](https://github.com/BerriAI/litellm/blob/main/litellm/proxy/proxy_server.py), [Pingora pooling](https://github.com/cloudflare/pingora/blob/main/docs/user_guide/pooling.md).

**Separate persistence and browser notifications from forwarding.** Bifrost uses one background writer with flush thresholds for count, estimated bytes and elapsed time. Its enqueue path drops entries when full and increments a counter. The source records that synchronous WebSocket callbacks stalled writes, while per-entry goroutines created excessive concurrency. Its batch-byte estimate can undercount data still held in parsed fields. Proposed: one SQLite writer, bounded batches, accounting for retained payload bytes, and separately bounded or coalesced dashboard updates. Display pending captures, persistence lag, write failures and dropped captures. [Bifrost writer](https://github.com/maximhq/bifrost/blob/dev/plugins/logging/writer.go).

**Bound the entire pipeline, including overflow handling.** LiteLLM acquires a logging semaphore before dequeueing in its normal worker loop. Its overload path can schedule delayed retry tasks and process extracted tasks outside that semaphore. Therefore, a bounded queue alone does not demonstrate bounded total memory. Proposed: a shared budget covering queued captures, active normalization, pending writes and browser notifications; avoid spawning extra tasks whenever a queue fills. Batching also reduces database work, as LiteLLM's production guidance explains. [Logging worker](https://github.com/BerriAI/litellm/blob/main/litellm/litellm_core_utils/logging_worker.py), [production guidance](https://docs.litellm.ai/docs/proxy/prod).

**Forwarding a stream does not make capture memory-free.** Helicone duplicates response chunks into an unbounded channel; its comment explicitly relies on concurrency and body-size limits elsewhere. The logger then collects the complete response. This is evidence about those components, not proof of unbounded gateway memory. Bifrost separately supports large-payload passthrough without always materializing complete bodies. Proposed: preserve native TypeSafe request/response semantics, cap retained bytes, and mark incomplete capture. Whether excess content is omitted, truncated or rejected needs a product decision; a capture cap need not imply an upstream payload cap. [Helicone body tee](https://github.com/Helicone/ai-gateway/blob/main/ai-gateway/src/types/body.rs), [logger](https://github.com/Helicone/ai-gateway/blob/main/ai-gateway/src/logger/service.rs), [Bifrost large payloads](https://github.com/maximhq/bifrost/blob/dev/core/providers/openai/large_payload.go).

**Measure observable behavior, not vendor rankings.** Bifrost provides a configurable mock upstream and separate fixed-rate and fixed-concurrency modes. LiteLLM explains that different payloads, client think times and concurrency depths produce non-comparable results. It also documents client-visible failures absent from gateway success metrics. Adopt a deterministic local mock and client-side measurement; use vendor results as methodology examples only. [Bifrost harness](https://github.com/maximhq/bifrost-benchmarking), [LiteLLM benchmarks](https://docs.litellm.ai/docs/benchmarks).

## Proposed benchmark matrix

Use identical native TypeSafe fixtures and mock responses across these configurations. Record exact versions, hardware, build profile, limits, warm-up, run duration and offered load.

| Configuration | Purpose |
|---|---|
| Client directly to mock | Establish transport and mock baseline |
| Proxy to mock, capture disabled | Measure forwarding cost |
| Proxy with metadata capture | Isolate collection and persistence |
| Proxy with full supported capture and grouping | Exercise intended observation behavior |
| Full capture with live dashboard | Include notifications and browser refreshes |
| Full capture plus historical reads/charts | Reveal read/write contention |
| Slow or unavailable storage; slow browser | Verify bounded behavior and visible gaps |

Cover one request with three answer types, repeated definitions, indexed batches, changing candidates, large bodies, upstream errors and cancellations. Exercise idle, steady, burst and sustained load; vary both offered request rate and in-flight concurrency. Add stream-specific cases only when supported.

Report client p50/p95/p99 completion latency, throughput, errors/timeouts, process RSS, queue entries/bytes, persisted requests and answers, dropped captures, and persistence-lag percentiles. Measure request receipt, upstream dispatch, upstream completion, client completion and durable commit separately. First-byte timing belongs alongside completion timing for streaming. Count usage/cost once per parent request. Compare complete latency distributions across runs; subtracting two p95 values is not a per-request overhead percentile.

## Decisions still needed

The user resolved the principal tradeoff in favor of continued inference with explicitly visible observation gaps when recording cannot keep up. Exact overflow accounting, shutdown and crash behavior still need definition. The supported workload should retain complete supported observations at hundreds of requests per second; permitted overload loss is not a substitute for achieving that capacity. Establish acceptable dashboard freshness, exact sustained and burst loads, history size, maximum captured body size, and target hardware before finalizing numerical acceptance thresholds. Implementation should not silently convert these unknowns into performance promises.

## Implementation follow-through

Observer now implements reusable upstream connections, streaming forwarding, one bounded capture budget across active calls and persistence, a batched writer and independent health reporting. Capture pressure skips observations without blocking forwarding. The dashboard refreshes bounded snapshots and stops polling hidden tabs.

Profiling also identified local storage costs: random request identifiers scattered writes across several B-tree indexes, and recurring-group queries repeatedly visited the same request history. Native capture now uses UUIDv7 identifiers, shared group aggregates and covering indexes; one redundant answer index was removed. The SQLite writer uses a 32 MiB page-cache target and readers use 8 MiB each, allocated on demand. The queue and active-capture budget both default to 1,024 entries. These are bounded buffering and indexing choices; their performance must be established by the local benchmark results, not by this source rationale. [UUIDv7 construction](https://docs.rs/uuid/latest/uuid/struct.Uuid.html#method.now_v7), [SQLite page-cache semantics](https://www.sqlite.org/pragma.html#pragma_cache_size).

Implementation measurements and supported-workload limits are now documented in [local performance results](../docs/performance.md). The original source review above remains historical design evidence.
