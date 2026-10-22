# Storage implementation

`src/store.rs` keeps the `Store`, `Filter`, `LabelOutcome` and `ExportSession`
interface used by the collector, credentials and HTTP handlers. The modules in
`src/store/` divide its implementation by responsibility:

- `connection.rs` opens encrypted and demo histories, checks schemas, migrates
  plaintext histories, applies file permissions and configures connections.
- `write.rs` persists records, imported annotations, local reviews and credential
  approvals. `retention.rs` owns maintenance and bounded history deletion.
- `filter.rs` builds anchored predicates and temporary request selections.
  `read.rs` pages request summaries and reconstructs complete records.
- `metrics.rs` owns numeric accumulation and timeline buckets. `groups.rs` owns
  group metadata, detail queries and snapshot-local version counts.
- `dashboard.rs` assembles the overview and chooses the existing sparse or broad
  strict-group query path.
- `export.rs` owns export sessions, page boundaries and CSV rendering.
- `value.rs` handles the shared stored-JSON and annotation helpers.

Transaction ownership stays with the calling operation. Record reconstruction
reads parents, answers and annotations from one snapshot. Writes, retention and
delete-all retain their existing outer transactions and checkpoint order. A
version-count cache belongs to one read snapshot, never to the `Store` instance.

Reader temporary payloads remain in memory. A filtered export may keep an
integer-only selection in a separate temporary file; each page opens a fresh
memory-only body reader and ends its transaction before returning to the HTTP
stream. The export session owns no transaction between pages.

Internal helpers are visible only within `store`. Existing regression modules
still exercise the same private methods and SQL, including snapshot isolation,
retention rollback, query work, legacy values and export limits.
