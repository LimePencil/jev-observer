# Storage, privacy and operating limits

## Inspect, import and export

Filter by source, model, time window or search. Open a request to inspect typed answers, probabilities, definitions and supplied application actions; add a local correct/incorrect/unknown review label. Open a question group to inspect its distribution, activity and separate definition versions. Pausing the visible dashboard or opening details holds the view while collection continues.

Import accepts Observer JSONL and the reviewed JevRouter decision-receipt format, up to 10,000 records and 8 MiB of import text. Explicit source event IDs prevent duplicate imports. Unknown event times stay unknown, with a separate import timestamp used to place records in history. Definitions changed by privacy filtering are conservatively isolated even if an imported record claims they were not redacted. JevRouter receipts remain application actions with their provenance; they do not become extra inference requests or invented token charges. Export the current filtered history as JSONL for portable records or CSV for spreadsheets. Delete history from Settings with an explicit confirmation. Importing or deleting history refreshes a paused view once while preserving its paused state.

Unknown usage, cost, confidence and outcomes remain unknown. Cost stays on the parent request even when it contains several answers. To calculate estimates, supply both `--input-price-per-million` and `--output-price-per-million` in USD; these are user-configured estimates, not invoices or an automatically maintained price list.

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

Retention maintenance runs every 30 seconds, so the record cap is a soft limit. It is not a disk-byte limit; deleting records does not necessarily shrink SQLite's allocated files. A shorter history may result from the record cap: at a constant 500 requests/sec, one million records represents about 33 minutes. Unsupported database versions are rejected before schema or journal-mode changes. For a complete filesystem backup, stop Observer cleanly before copying its database and any remaining SQLite sidecar files. JSONL exports provide portable application records, not a full database backup.

Capture slots must cover concurrent calls plus queued writes, not just requests per second: for example, 500 requests/sec with a one-second upstream needs more than 500 slots. The default 1,024 slots allows headroom for this workload. Buffers grow on demand; the configured request/response body budget is capped at 512 MiB, excluding HTTP buffers and parsed-record overhead. Slower upstreams or larger payloads need their own capacity test.

Request and response capture is bounded independently of forwarding: a large body can be forwarded in full while its saved observation is incomplete. A full capture budget or queue drops observation work. Recording/storage failures appear in collection health; they do not intentionally wait on forwarding. Health counters and the last-gap timestamp describe **the current process** and reset on restart. Retained history may therefore be incomplete even when a newly started process shows no drops.

Ctrl-C or SIGTERM cancels upstream work, allows up to 10 seconds for HTTP connections to finish, then drains queued captures. A separate watchdog limits total shutdown to 15 seconds, including runtime cleanup; if work remains blocked, it prints that pending captures may be lost and exits with status 2. Exceeding the HTTP grace also returns a nonzero exit status after draining. Abrupt termination or a forced deadline can lose observations; there is no durable audit guarantee. Calls cannot pass through a stopped Observer process.

Raw request state is discarded from saved history unless explicitly enabled. Question definitions, answers and supported extension fields are retained and may themselves contain sensitive content. Known credential fields and the forwarded secret are redacted; use repeated `--redact-key KEY` options for additional field names. Redaction does not promise to identify every sensitive value. Live SQLite history is encrypted with SQLCipher using `JEV_OBSERVER_DB_KEY`. On first startup with an existing plaintext database, Observer migrates it before serving requests; stop older Observer processes first. Old backups and deleted disk blocks may still contain plaintext. Demo history is synthetic and remains plaintext. JSONL/CSV downloads are deliberate plaintext exports; store or share them with appropriate protection.


Windows uses Ctrl-C or Ctrl-Break for graceful shutdown. The default `.jev-observer` directory receives a private inheritable current-user ACL before database creation. Custom Windows database directories must already restrict access to the current user, SYSTEM and administrators. Dashboard token files use protected current-user ACLs; permissive files and reparse points are rejected.

See [application connection](connection.md) for database-key setup, dashboard authentication and provider credentials.
