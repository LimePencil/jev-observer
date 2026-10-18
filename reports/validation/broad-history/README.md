# Broad history query indexes (#102)

The candidate adds two automatically maintained SQLite indexes. `answer_overview` encodes canonical 73-byte hexadecimal group IDs as 32-byte BLOBs and retains all other index columns in the original order. Noncanonical IDs remain TEXT, so identities and deterministic accumulation order are preserved. `answer_key(key,request_id)` supports at most 513 distinct-key seeks: when there are at most 512 keys, substring matching runs once per key and matching parent IDs come from indexed equality probes. More keys or over 128 KiB of matching key text retain the original scan; narrow scopes retain correlated lookups. No result sampling or deadline changes are involved.

## Related implementations consulted

- [Datasette: speeding up facets with indexes](https://docs.datasette.io/en/latest/facets.html#speeding-up-facets-with-indexes) informed indexing repeated dimensions used by overview filters.
- [DuckDB: Efficient SQL on Pandas](https://duckdb.org/2021/05/14/sql-on-pandas), particularly projection and filter pushdown, informed reducing data read before aggregation. This change keeps the existing SQLCipher engine and exact Rust aggregation; it does not add DuckDB.
- [SQLite query planning](https://www.sqlite.org/queryplanner.html#covering_indexes) informed preserving ordered index scans without reading saved request/answer JSON bodies.

## Controlled Store comparisons

`store-100k.json` compares frozen v0.3.0 Store source (`02eb5ba`) with the candidate on one encrypted fixture of 100,000 parents and 300,000 answers. Three alternating pairs per all/search/empty-error scope matched complete dashboard JSON. The fixture and every owned helper were removed. Timings are descriptive shared-host observations, not a latency guarantee.

| Scope | Before median | After median | Before VM steps | After VM steps |
| --- | ---: | ---: | ---: | ---: |
| All | 442.10 ms | 367.53 ms | 4,804,676 | 5,104,754 |
| Search | 619.61 ms | 516.25 ms | 10,079,771 | 8,780,003 |
| Empty errors | 141.27 ms | 142.68 ms | 1,419,736 | 1,419,736 |

The expression index introduces one deferred-seek VM instruction per answer, even though its selected columns are in the index. Thus all-history VM steps increase; smaller index reads are the intended improvement. Search also reduces VM work. Unit regressions compare exact selected IDs, preserve special/noncanonical group identities and duplicate-parent ordering, bound dictionary discovery, and exercise the high-cardinality fallback.

## Upgrade and storage cost

`migration-100k.json` creates/seeds the old schema, opens it with the candidate, then repeats nine exact-output pairs using both implementations. Index creation plus normal startup maintenance took 6.51 seconds in that shared-host run. Database size grew from 506,785,792 to 540,876,800 bytes (6.73%). Startup index creation is one-time and can take substantially longer on larger histories; it requires additional disk space, including temporary index-building space/WAL. Both indexes add ingestion and deletion work. Original tables and indexes remain readable by older versions; SQLite's built-in functions maintain the new indexes for old writers as well.

The measured fixture has three repeated definitions and small synthetic answers. High-cardinality histories and arbitrary filter distributions are not covered by these latency observations.

## Reproduction boundary

The deterministic regressions are retained in `src/dashboard_tests.rs` and run with `cargo test`. Large-fixture Store/HTTP harnesses were session-local; their source and executable hashes are recorded in the archived reports, but those harnesses and binaries are not bundled here. Paths containing `<repo>` identify the original run and are not links to bundled files. Public copies normalize host paths while retaining the SHA-256 of their private source reports. Store comparison snapshots precede the embedded-NUL and oversized-key hardening; neither boundary occurs in that synthetic fixture. The final HTTP evidence identifies the final runtime source explicitly.

## Final million-record HTTP check

`million-final.json` measures runtime source `f7f954f96be0f8cd1d4150b4f0004ba6d9a9d46e`, binary SHA-256 `ad6564a5d89e73195d62cd3fb1c9b7af5e8f66c151a82873b9a32e67d409dfc5`. All 26 checks passed on 1,000,000 encrypted parents / 3,000,000 answers: exact aggregate/timeline/group counts, inclusive dates, advancing pages, four full sampled records, no remote inference, normal process exits and complete cleanup. The earlier `million-initial.json` run also passed; it precedes the special-ID/key-memory hardening.

| HTTP scope | First request | Repeat |
| --- | ---: | ---: |
| All history | 14.34 s | 4.75 s |
| Source | 4.84 s | 4.82 s |
| Answer-key search | 9.19 s | 6.45 s |
| Empty errors | 1.49 s | 1.52 s |

The second request page took 4.61 s. Every measured request completed below the original 15-second boundary. The 45-second dashboard timeout and slow-query notice remain useful for other workloads. There was no explicit OS-cache reset, and comparison with historical million-record timings is not a controlled speedup or a tail-latency/capacity guarantee. The controlled comparison is the paired Store evidence above.

The final runtime passed 194 Rust tests (two intentionally ignored) and Clippy with warnings denied. `upgrade.json` records 36 passing published-v0.1.0 upgrade/rollback/backup checks. Published-v0.3.0 keyring/database interoperability also runs in CI through the isolated harness introduced by #127. No published release asset is replaced by this change.
