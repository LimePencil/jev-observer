# Local application contract

Implementation baseline for the current goal, September 22, 2026. User-confirmed: all dashboard capabilities together, forwarding continues during recording pressure, hundreds of requests per second. Working defaults: Linux first, mixed workloads, full state opt-in, 7-day detailed retention with an adjustable record cap, 500 requests/s sustained and 1,000 requests/s burst test points. These are implementation choices and targets, not measured claims.

## Modules and boundaries

- `src/model.rs`: captured wire data, normalization, deterministic grouping, privacy, sample fixtures, import conversion.
- `src/store.rs`: SQLite migrations, batched writes, dashboard/detail queries, export, labels and deletion.
- `src/collector.rs`: bounded capture reservations and a single background normalization/database writer, independent health counters.
- `src/server.rs`, `src/config.rs`, `src/main.rs`: HTTP proxy, local API, bundled UI, CLI and shutdown.
- `ui/`: React + Vite browser app with local assets and Taste Skill visual guidance. No remote UI resources.

The module boundary uses `serde_json::Value` for normalized records and API responses. JSON object insertion order must be preserved for presentation fingerprints.

## Wire capture contract

`model::Capture` fields: `id: String`, `timestamp: i64` (Unix milliseconds), `source: String`, `task_version: Option<String>`, `adapter: Option<String>`, `status: u16`, `duration_ms: f64`, `request: Vec<u8>`, `response: Vec<u8>`, `capture_complete: bool`, `transport_error: Option<String>`, `secret: Option<String>` (forwarded credential, memory only), `local_token: Option<String>` (registered-key client token, memory only).

`model::NormalizeOptions`: `capture_state: bool`, `redact_keys: Vec<String>`, `input_price_per_million: Option<f64>`, `output_price_per_million: Option<f64>`. `normalize(&Capture, &NormalizeOptions) -> Value`. `sample_records() -> Vec<Value>` creates synthetic records with relative recent timestamps and varied sources/types/versions/errors. `import_records(text: &str, format: &str, options: &NormalizeOptions) -> anyhow::Result<Vec<Value>>`, supporting `observer-jsonl` and an explicitly documented real existing log adapter. Exported records can be reimported using explicit provenance IDs; reject invalid records, do not trust supplied grouping fingerprints.

Record JSON (all keys present, absent data is null):

```
{schema_version:1,id,event_kind,timestamp,imported_at,timestamp_basis,source,provider,model,requested_model,status,duration_ms,
 input_tokens,output_tokens,cost_usd,cost_basis,source_event_id,import_format,
 capture_complete,state_retained,state,transport_error,sample:false,
 answers:[{key,kind,group_id,definition_id,presentation_id,candidate_id,
   task_version,definition,value,probabilities,confidence,valid,error,
   family_id,family_name,adapter,instance_ref,mapping_reason,raw_answer,definition_redacted}],
 actions:[],labels:[]}
```

`value` is Choice string / Score number / Noul probability. `kind` is lowercase `choice|score|noul|unknown`. Status errors and invalid/missing answers do not enter valid distributions. Imported application actions have `event_kind=application_action`, do not enter inference accounting, and retain unknown original timing/transport facts as null with a separately labeled import timestamp. Fingerprints are source/key/definition/presentation/task scoped; a family is separate from strict identity. Native headers are never stored. Pricing is only an explicitly configured estimate; missing usage/rates stay unknown. Sample cost is labeled synthetic.

Observer imports also preserve null event timestamps, assigning an import timestamp when absent or null and setting `timestamp_basis=import`. Token counts must be nonnegative signed 64-bit integers; out-of-range native usage stays unknown and cannot establish estimated cost, while invalid imported counts are rejected. String credentials inside sensitive arrays/objects are collected for redaction of echoes elsewhere in the record. Answer and review keys preserve arbitrary strings, including empty keys. A valid answer requires an explicit supported type in its original question definition; response metadata cannot supply a missing definition type.

## Storage interface

`Store::open(path: &Path, retention_days: u32, max_records: usize) -> Result<Store>`, cloneable. `is_empty() -> Result<bool>` (indexed existence check), `writer_connection() -> Result<Connection>`, `write_batch(conn: &mut Connection, records: &[Value]) -> Result<usize>`, `dashboard(&Filter) -> Result<Value>`, `request(id: &str) -> Result<Option<Value>>`, `group(id: &str, &Filter) -> Result<Option<Value>>`, `export_page(&Filter, format: &str, after: i64, through: Option<i64>) -> Result<(String,i64,i64,bool)>`, `add_label(id: &str, key: &str, label: &str) -> Result<()>`, `delete_all() -> Result<()>`.

`Filter` (Deserialize + Default): `source, model, window, group, status, search: Option<String>`. `window`: `1h|24h|7d|all`, default `24h`. `status=error` filters failures. Group filters accept a strict group or explicit family ID; an empty group string is equivalent to no group filter. An internal, non-deserializable `as_of` timestamp freezes a dashboard or export time window. Request details and each export page use a read transaction so parents, answers and labels share a snapshot. Exports fix the maximum sequence and release each page's transaction before download; concurrent retention/deletion and label edits may still change later pages. The server permits one export at a time, and downloads use browser streaming. CSV reads saved parent metrics and the transactionally maintained answer count without reconstructing answer definitions or reviews; JSONL retains complete records.

Dashboard search/group filters select matching parent sequence IDs once into a connection-local temporary table and reuse that selection within the read snapshot. The main database stays read-only; temporary storage uses a 2 MiB pager-cache target and stores only integer IDs. Broad searches scan answer keys once; narrow scopes retain per-parent lookups. Matching a question selects the whole parent request, preserving request-level usage and all its answers. Broad filtered feeds walk the timestamp index and stop at the latest 100 matches; small selections retain direct row lookups.

The `request_dashboard` covering index stores scalar metrics and the original event timestamp alongside filter fields, avoiding repeated reads and JSON parsing of saved bodies. It is created/backfilled on opening an existing supported v1 database and maintained transactionally by SQLite. One request scan computes exact totals, timeline buckets and nearest-rank latency percentiles, plus a snapshot-local map of selected parent IDs to timestamps. Strict groups stream their covering answer index against that map, with compensated means and only the latest 100 group results retained. Families remain separately aggregated and the final merged list remains bounded to 100. The parent map and percentile buffer scale with selected request count; neither survives the response or caches history across mutations.

## Collector interface

`Collector::start(store: Store, options: NormalizeOptions, capture_limit: usize, capture_slots: usize, queue_capacity: usize) -> Collector`.
Cloneable collector: `reserve_capture() -> Option<OwnedSemaphorePermit>`, `submit(Capture, OwnedSemaphorePermit)` (never awaits), `record_forwarded()`, `record_skipped()`, `health() -> Value`, `async shutdown()`.
One reservation covers at most `capture_limit` bytes of each wire body and remains held until normalization/persistence completes. No request waits for a reservation. A missing reservation means capture is skipped and counted; the proxy still forwards the full body. A full queue drops the capture and records the gap. One dedicated writer does normalization and batched writes off the async forwarding runtime. Defaults: 256 KiB per body, 1,024 slots, 1,024 queued events. Graceful shutdown drains; failures are counted independently of SQLite.

## HTTP and UI API

Proxy: `POST /v1/systemone` to one configured upstream origin/path (default `https://api.typesafe.ai/v1/systemone`). Accept bearer key from caller or `TYPESAFE_API_KEY` in the process environment; fallback requests require `application/json`. Disable redirects and automatic retries; stream full bodies, separately capture bounded copies; strip hop-by-hop, local cookies and `x-observer-*` headers, and discard upstream `Set-Cookie`. Local metadata headers: `x-observer-source`, `x-observer-task-version`, `x-observer-adapter` (opt-in `jgrep-v1`). Bind loopback only. Upstream configuration is CLI-owned, never a per-request URL; remote URLs require HTTPS and HTTP is limited to loopback development endpoints.

Registered provider keys: `PUT /api/credentials` accepts a provider key and a session/system persistence choice, returns a newly generated local client token once, and rotates any previous token. `GET /api/credentials` returns only configuration status; `DELETE` removes the registered key. The local token is accepted as the SDK's Bearer credential, validated before forwarding, then replaced with the provider key. A token with the reserved local prefix that does not match is rejected without contacting the upstream. System persistence uses an OS credential entry scoped to the canonical workspace database path. SQLite stores only a hash of the random token as approval for restoring that entry; changing or removing a key revokes approval before trying to delete an obsolete entry. Neither key nor token is written to SQLite or exports, and both are redacted if echoed in captured traffic. System credential operations run off the async HTTP executor. Demo mode cannot configure or forward provider credentials.

- `GET /api/dashboard` + Filter query -> `{generated_at,sample,sources:[],models:[],summary,timeline,groups,requests,health}`. Server adds live collector health to the Store result.
- `summary`: `{request_count,action_count,answer_count,error_count,p50_ms,p95_ms,input_tokens,output_tokens,cost_usd,cost_known_requests}`. Unknowns null; known-cost sum is explicitly partial when coverage is incomplete.
- `timeline`: `[{timestamp,requests,errors,mean_latency_ms,cost_usd}]` (time buckets).
- `groups`: `[{id,name,key,kind,source,definition_id,presentation_id,definition,task_version,family_id,family_name,adapter,answer_count,valid_count,request_count,last_seen,version_count,distribution:[{label,count}],mean_value,mean_confidence}]`.
- `requests`: newest 100 summaries `{id,timestamp,source,model,status,duration_ms,input_tokens,output_tokens,cost_usd,cost_basis,answer_count,capture_complete,sample}`. Summary and chart counts cover the full selected window, not this limited feed.
- `health`: `{process_id,sample_sequence,forwarded,captured,persisted,dropped,truncated,write_failures,maintenance_failures,maintenance_healthy,last_maintenance_at,queue_depth,queued_bytes,last_persisted_at,last_gap_at,lag_ms,capture_limit,capture_slots}` (extend as needed). Counters cover the current process. A stable collector `process_id` and increasing `sample_sequence` identify the order of health samples shared by the independent health and dashboard endpoints. `truncated` counts all incomplete captures, including cancellation and unsupported content encoding. Retention failures are reported separately from dropped observations.
- `GET /api/requests/{id}` -> complete normalized record, 404 if absent.
- `GET /api/groups/{id}` + Filter -> `{group,versions:[],timeline:[],requests:[],answers:[]}`. Answers include `{request_id,timestamp,imported_at,source,key,value,valid,probabilities,confidence,label}`; unknown event timestamps remain null. Select the latest 100 answers by history time, breaking timestamp ties by newest parent sequence. Bound detail lists and disclose limits. Every section, including version summaries, retains the caller's parent filter; the detail group further narrows that scope.
- `POST /api/import` JSON `{text,format}` -> `{imported,duplicates}`; queue-independent local import, explicit IDs deduplicate. Limit decoded UTF-8 import text to 8 MiB and 10,000 records; allow JSON escaping overhead within a separately bounded transport body.
- `GET /api/export?format=jsonl|csv` + Filter -> downloadable local records.
- `POST /api/requests/{id}/label` JSON `{key,label}` (`correct|incorrect|unknown`) -> `{ok:true}`.
- `DELETE /api/data` -> `{ok:true}`; UI requires a deliberate confirmation.
- `GET /api/settings` -> public configuration only: `{demo,capture_state,retention_days,max_records,capture_limit,upstream,version}`. Never expose credentials.

Dashboard assets and local API endpoints require HTTP Basic authentication with username `observer` and a random workspace token held in an owner-only sibling of the database (`*.access-token`). Registered local client tokens authenticate proxy calls; direct provider keys and the process environment fallback also require `X-Observer-Access` containing the workspace token. Live SQLite databases use SQLCipher with an externally supplied 64-character hexadecimal `JEV_OBSERVER_DB_KEY`; existing plaintext databases migrate before the listener starts. Demo data and explicit JSONL/CSV exports remain plaintext. All local API mutations require `X-Observer-Request: 1`, same-origin validation when Origin is supplied, and allowed Host validation (including reads). Cross-site browser reads identified by Fetch Metadata are rejected. No CORS wildcard. Bound import size. Async HTTP handlers execute SQLite work in `spawn_blocking`.

CLI: `--demo`, `--port` (default 8765), `--db`, `--upstream`, `--capture-state`, `--capture-limit`, `--capture-slots`, `--queue-capacity`, `--retention-days`, `--max-records`, optional explicit input/output rates. Demo uses an isolated database and no provider calls; existing history, including action-only imports, survives restarts without reseeding. Startup uses an existence check rather than aggregating the dashboard. UI build output embedded into the executable, with useful fallback development instructions if unavailable.

## Dashboard behavior

One overview: shared source/model/window/search filters; summary strip; volume/latency timeline; question distribution and searchable recurring groups; live request/failure feed; persistent capture status; request/group detail drawer with version comparison and labels. Pause visible updates while investigating, never ingestion. Poll without overlapping requests, with cancellation and hidden-tab pause. Successful queries count toward a one-second start-to-start cadence, with a minimum 100 ms idle period after slow queries; failures wait one second before retrying. Accessible keyboard/focus behavior, light/dark themes, responsive layout, functional import/export and settings/data controls. Show all unknowns, denominators, cost coverage and incomplete captures honestly. No fake live activity, claims of measured accuracy from confidence, or simulated backend on the production path.

Filter catalogs and the active source/model remain visible during scoped refreshes, failures, and retention changes. Health follows server sample order within a process; a delayed response must not hide a newer reported gap. Request-start ordering distinguishes process transitions and provides a fallback for older responses without sample metadata.
