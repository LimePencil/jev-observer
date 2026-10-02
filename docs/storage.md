# Storage, privacy and operating limits

## Inspect, import and export

Filter by source, model, time window, custom date range or search. Request and question-group lists have additional pages; searching groups searches retained groups beyond the first page. Summary totals cover the full selected request scope. Timelines cover that range with at most 168 adaptive time buckets, including empty intervals. Records with unknown original event times remain outside the timeline.

Open a request to inspect typed answers, probabilities, definitions and supplied application actions; add a local correct/incorrect/unknown review label. Open a question group to compare definition versions, answer distributions, review counts, latency, usage and known cost within the same filters. These comparison metrics are computed when opening group details rather than during every overview refresh. Unlabeled answers remain separate from explicit unknown reviews. A request's cost is counted once within each version's metrics; groups or versions sharing requests must not have their costs added together. Detail lists remain bounded to 100 entries, with the viewed version and recently active versions preferred. Pausing the visible dashboard or opening details holds the view while collection continues.

Failures include unsuccessful HTTP statuses and recorded transfer errors, including a body transfer that fails after HTTP 200 headers. The observed HTTP status remains available. Incomplete capture caused only by a size limit or unsupported encoding does not by itself imply that the application request failed.

Malformed or out-of-range answers stay invalid. A Score that differs from the weighted displayed probabilities by more than 0.001 carries a consistency warning while preserving its reported score and probabilities. Otherwise valid warned answers remain in distributions, and version metrics report their count. A warning does not establish the cause of the discrepancy or guarantee that the provider result is correct. The new validation and cost rules apply when capturing or importing records; an upgrade preserves previously saved observations without reclassifying them.

Import accepts Observer JSONL and the reviewed JevRouter decision-receipt format, up to 10,000 records and 8 MiB of import text. Explicit source event IDs prevent duplicate imports. Unknown event times stay unknown, with a separate import timestamp used to place records in history. Definitions changed by privacy filtering are conservatively isolated even if an imported record claims they were not redacted. JevRouter receipts remain application actions with their provenance; they do not become extra inference requests or invented token charges. Export the current filtered history as JSONL for portable records or CSV for spreadsheets. Delete history from Settings with an explicit confirmation. Importing or deleting history refreshes a paused view once while preserving its paused state.

Unknown usage, cost, confidence and outcomes remain unknown. Cost stays on the parent request even when it contains several answers. The OpenRouter adapter recognizes a finite, nonnegative `usage.cost` as provider-reported USD and gives it precedence over configured estimates. Other upstreams' similarly named fields remain extension data because their units have not been established. To calculate estimates, supply both `--input-price-per-million` and `--output-price-per-million` in USD. Saved records distinguish `provider_reported` from `configured_estimate`; neither is a verified invoice or an automatically maintained price list. A missing cost remains unknown; an explicitly reported zero remains zero.

## Storage and operating limits

| Setting | Default |
|---|---|
| Listener | `127.0.0.1:8765`; change the port with `--port` |
| Database | `.jev-observer/observer.sqlite`; change with `--db` |
| Retention | `--retention-days 7` |
| Record cap | `--max-records 1000000` |
| Captured bytes | `--capture-limit 262144` per request body and per response body |
| Captures across active requests, queue and writer | `--capture-slots 1024` |
| Pending queue capacity | `--queue-capacity 1024` |
| Input-state persistence | Off; opt in with `--capture-state` |

Retention maintenance runs every 30 seconds, so the record cap is a soft limit. It is not a disk-byte limit; deleting records does not necessarily shrink SQLite's allocated files. A shorter history may result from the record cap: at a constant 500 requests/sec, one million records represents about 33 minutes. Unsupported database versions are rejected before schema, journal-mode or plaintext-encryption changes. Supported existing histories receive a transactional query-index upgrade. For a complete filesystem backup, stop Observer cleanly before copying its database and any remaining SQLite sidecar files. Keep its database key and dashboard token available separately. JSONL exports provide portable application records, not a full database or operating-system credential backup.

Capture slots must cover concurrent calls plus queued writes, not just requests per second: for example, 500 requests/sec with a one-second upstream needs more than 500 slots. The default 1,024 slots allows headroom for this workload. Buffers grow on demand; the configured request/response body budget is capped at 512 MiB, excluding HTTP buffers and parsed-record overhead. Slower upstreams or larger payloads need their own capacity test.

Request and response capture is bounded independently of forwarding: a large body can be forwarded in full while its saved observation is incomplete. A full capture budget or queue drops observation work. Recording/storage failures appear in collection health; they do not intentionally wait on forwarding. Health counters and the last-gap timestamp describe **the current process** and reset on restart. Retained history may therefore be incomplete even when a newly started process shows no drops.

Collection health also retains the latest storage-failure time, operation and sanitized category: busy database, full storage, denied permissions, unreadable database, storage I/O or another database error. Raw SQL, record values and filesystem paths are excluded. This is the last observed failure, which can remain visible after recovery; current retention health and later successful writes provide recovery context.

Ctrl-C or SIGTERM cancels upstream work, allows up to 10 seconds for HTTP connections to finish, then drains queued captures. A separate watchdog limits total shutdown to 15 seconds, including runtime cleanup; if work remains blocked, it prints that pending captures may be lost and exits with status 2. Exceeding the HTTP grace also returns a nonzero exit status after draining. Abrupt termination or a forced deadline can lose observations; there is no durable audit guarantee. Calls cannot pass through a stopped Observer process.

Raw request state is discarded from saved history unless explicitly enabled. Question definitions, answers and supported extension fields are retained and may themselves contain sensitive content. Known credential fields and the forwarded secret are redacted; use repeated `--redact-key KEY` options for additional field names. Redaction does not promise to identify every sensitive value. Live SQLite history is encrypted with SQLCipher using `JEV_OBSERVER_DB_KEY`. On first startup with an existing plaintext database, Observer migrates it before serving requests; stop older Observer processes first. Old backups and deleted disk blocks may still contain plaintext. Demo history is synthetic and remains plaintext. JSONL/CSV downloads are deliberate plaintext exports; store or share them with appropriate protection.

Persisted provider-key rotation stages a new operating-system credential entry before switching SQLite's approval digest. A failed approval leaves the previously approved entry available; obsolete entries are removed after the switch. Removing a credential revokes approval before operating-system cleanup. Version 0.2.0 can load older workspace entries, but version 0.1.0 cannot load the new entry layout after rotation. See [upgrade and downgrade guidance](installation.md#history-backups-and-downgrades).


Windows uses Ctrl-C or Ctrl-Break for graceful shutdown. The default `.jev-observer` directory receives a private inheritable current-user ACL before database creation. Custom Windows database directories must already restrict access to the current user, SYSTEM and administrators. Dashboard token files use protected current-user ACLs; permissive files and reparse points are rejected.

See [application connection](connection.md) for database-key setup, dashboard authentication and provider credentials.
