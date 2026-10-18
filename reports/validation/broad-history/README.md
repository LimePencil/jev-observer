# Broad history query indexes (#102)

The candidate adds two automatically maintained SQLite indexes. `answer_overview` encodes canonical 73-byte hexadecimal group IDs as 32-byte BLOBs and retains all other index columns in the original order. Noncanonical IDs remain TEXT, so identities and deterministic accumulation order are preserved. `answer_key(key,request_id)` supports at most 513 distinct-key seeks: when there are at most 512 keys, substring matching runs once per key and matching parent IDs come from indexed equality probes. More keys retain the original scan; narrow scopes retain correlated lookups. No result sampling or deadline changes are involved.

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
