# Independent validation of the follow-up fixes

Validated September 22, 2026, using fresh reproductions written by reviewers who did not implement these fixes. The existing regression suite was not used as proof of the claims. All data was synthetic, databases disposable, and upstream traffic confined to a local mock.

The reported failures reproduce before the fixes and the corresponding scenarios behave correctly afterward. Some claims require narrower wording and should not all be treated as ordinary traffic failures.

| Claim | Independently observed before → after | Assessment |
|---|---|---|
| Valid imports rejected after JSON encoding | 5,000 valid records: 8,320,000 bytes of text, 8,755,037 transport bytes. HTTP 413 → HTTP 200 with all 5,000 imported. Exactly 8 MiB also rejected → accepted; 8 MiB + 1 stays rejected. | Confirmed normal import boundary bug. |
| Imports with unknown event timestamps fail | `timestamp:null` with absent/null `imported_at`: HTTP 503 → successful import with null event time and a separate import time. | Confirmed. An entirely omitted `timestamp` field remains invalid (400 before and after). |
| Credential echoes leak into saved history | Credentials inside sensitive arrays/objects, and normalized echoes of a multi-space Bearer header, remain readable before → redacted afterward. Ordinary scalar secrets and single-space Bearer controls already redact correctly. | Confirmed conditional privacy bugs. Requires a sensitive value to be echoed elsewhere; these tests do not imply routine headers were being stored. |
| Request/export reads mix snapshots | Overlapping HTTP reads with `DELETE /api/data` produces incomplete records before: 10/18 request reads, 14/18 JSONL exports, and 12/18 CSV exports. After: zero incomplete records across the same 54 trials. | Confirmed through unmodified binaries, plus a deterministic source harness. The valid 2 MiB metadata fixture widens the race window; these rates are not production incidence. |
| Group history ordering/timestamps are inconsistent | With 105 requests at one timestamp, request feed returns 104…005 but old answers return 000…099; after, both return 104…005. Old group answers also substitute import time for a null event timestamp; after, the fields stay separate. | Confirmed through unmodified HTTP binaries. The timestamp error affects the group API; its timestamp field is not currently rendered in the group answer list. |
| Oversized token counts produce inconsistent costs | Counts above signed 64-bit range remain in old detail JSON and establish estimated cost, while the dashboard loses the counter. After, native count/cost are unknown; oversized imported counters are rejected. | Confirmed pathological-input bug. Requires more than 9,223,372,036,854,775,807 tokens in one field; classify as defensive validation, not plausible normal usage. |
| Clipboard failures/feedback | Native browser permission denial is caught before, but its message is inside an `aria-hidden` root and absent from the accessibility tree. A missing clipboard API throws before. After, feedback is exposed inside the dialog without an uncaught error. | Confirmed. Successful native copying works before and after. Missing API was explicitly emulated, not observed as the default loopback Chromium environment. |
| Delayed deletion closes a newer panel | Hold a real successful DELETE response, leave Settings, open Connect or reopen Settings, then release: old code closes the new panel; current code preserves it. Staying in the original Settings panel closes normally in both. | Confirmed conditional UI race. Response delay is controlled; the request and response come from the real API. |

## Evidence and limits

The recorded backend and group HTTP tests compared these unmodified executables. These hashes identify the historical test inputs, not a subsequent build of the final commit:

- Before: `.jev-observer/before-refresh-observer`, SHA-256 `ec5f0ae0f5ca6ef1f8267bb5dc4560747096736638a01c67a6bd5ec445d7c047`.
- After: `target/release/jev-observer`, SHA-256 `fd4c31dd90ada212e59fab18cdec59928a82b8f382c595bbe2f7eb81e0d3869c`.

The saved baseline's exact source provenance was not rebuilt independently; the HTTP conclusions are direct comparisons between these identified executables. Source and UI comparisons separately use the identified Git revision.

The UI comparison used separate production builds of revision `90e3cfb625d85b4279977a690903b7790468f7ed` and the working-tree UI sources identified in the [source manifest](ui/source-manifest.json). Both used the recorded after backend with a disposable database. Native permission denial and permission-granted copying were tested in Chromium; unavailable API was an explicit capability simulation.

The HTTP storage race uses only real import/read/delete endpoints on disposable databases. A valid record contains one answer, one label and 2 MiB of ordinary metadata. Read and deletion requests overlap at six short delays, with three trials per delay for each read route. The old binary returns retained parents with missing answers/labels; old CSV reports zero answers. One old JSONL export is legitimately empty because deletion wins before its read. The current binary returns complete records in every trial. This is an intentionally widened race window, not a typical-workload failure-rate measurement.

The additional deterministic storage comparison copied exact baseline/working-tree `store.rs` sources, identified by hashes in the [results manifest](storage/snapshots-results.json), and added one hook after the parent SELECT. It waited for a separate writer to commit, then resumed the unchanged `request()`/`export_page()` code. Delete-and-reinsert additionally demonstrated mixed generations, but was not needed to establish the deletion defect. Completed reads allowed a truncating WAL checkpoint, confirming that the fix released its snapshot.

The original test `record_reads_keep_answers_and_labels_in_their_parent_snapshot` had a coverage gap: it still passed when both public read transactions were removed in an isolated copy. It opened its own transaction before calling the helper, so its passing result did **not** independently validate the public request/export fix. The [historical mutation log](test-quality/existing-regression-with-fix-removed.log) records that result. The independent public-API harness caught the failure. Product source and tests were unchanged during that historical validation; subsequent regression-test changes are separate from this evidence.

## Subsequent regression coverage

After the gap was identified, the original helper test was replaced with five tests that call the public methods: request reads during deletion and label edits, JSONL exports during deletion and label edits, and CSV export during deletion. A test-only thread-local hook schedules a committed writer change after the parent read. The tests verify both the original snapshot and a subsequent read of the committed change, plus successful snapshot release through a truncating WAL checkpoint.

A separate [mutation audit](test-quality/public-snapshot-regression/results.json) compiled the exact new tests against isolated source copies. The tested `store.rs` hash was `804df4d6a909dff9208554f5cb0eb2418fb832d794db781a84a0b686282d28d6`. This audit proves that the new tests detect removal of either public transaction wrapper; it does not change the historical HTTP/UI results or their executable hashes.

| Source variant | Passed | Failed | Test log |
|---|---:|---:|---|
| Public transaction wrappers present | 5 | 0 | [Current](test-quality/public-snapshot-regression/current.test.log) |
| Request transaction removed | 3 | 2 request tests | [Request mutation](test-quality/public-snapshot-regression/without_request_transaction.test.log) |
| Export transaction removed | 2 | 3 export tests | [Export mutation](test-quality/public-snapshot-regression/without_export_transaction.test.log) |
| Both transactions removed | 0 | 5 | [Combined mutation](test-quality/public-snapshot-regression/without_both_transactions.test.log) |

The audit changed only source copies and also relaxed the private helper's parameter to `&Connection` so the deliberately broken variants compiled. Workspace source and the existing release executable were unchanged during the audit. The [audit script](test-quality/public-snapshot-regression/check_mutations.py) is retained with its prerequisites in the [reproduction instructions](README.md).

## Retained evidence

The following files are committed with this report and remain available in a clone. Public JSON results and historical mutation logs replace the original absolute checkout prefix with `<repo>`, preserving recorded observations. The [evidence manifest](evidence-manifest.json) retains original hashes and byte counts and separately identifies normalized copies. Screenshots, complete accessibility trees, binaries, databases, dependency caches, and duplicated source/build trees are omitted. Browser results retain the relevant accessibility-tree observations.

- [Backend script](backend/reproduce_backend.py) and [raw HTTP evidence](backend/evidence.json): 38 probes per executable.
- [Browser script](ui/reproduce.mjs), [source manifest](ui/source-manifest.json), and [browser results](ui/results.json): eight clipboard and six delayed-deletion scenarios.
- [Group script](storage/check_groups.py) and [group results](storage/groups-results.json).
- [HTTP concurrency script](storage/check_http_races.py) and [HTTP concurrency results](storage/http-races-results.json): 54 trials per executable.
- [Deterministic storage script](storage/check_snapshots.py) and [storage results](storage/snapshots-results.json): 12 instrumented-source scenarios.
- [Existing regression passing with its fix removed](test-quality/existing-regression-with-fix-removed.log).

The [reproduction instructions](README.md) describe prerequisites and the unavailable historical binary provenance. `python3 reports/validation/verify-evidence.py` checks the retained evidence offline; it does not rerun the application or establish that a later build behaves identically.
