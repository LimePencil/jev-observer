# 0.2.0 release readiness

Updated October 2, 2026. Work is tracked in [issue #1](https://github.com/LimePencil/jev-observer/issues/1), on `release/local-models-and-reliability`. The source candidate is 0.2.0; the published release remains 0.1.0. No tag or release has been published by this work.

## Implemented scope

| Area | Candidate behavior and verification |
|---|---|
| Local System One models | Explicit `--upstream-auth none` supports loopback models while preserving Observer authentication. `--provider laya` handles its typed-answer formats and precision. Three actual Laya CPU calls produced nine valid answers; all 16 smoke checks passed. |
| Unsupported schemas | Read-only preflight rejects unsupported plaintext history before migration. Existing encrypted history is checked before schema or journal changes. Upgrade tests compare database bytes before and after rejection. |
| Failed transfers | HTTP errors and transport errors share failure counts, filters, timeline and badges. A broken response after HTTP 200 remains a failure; capture-only gaps remain separate. |
| Score consistency | Structurally valid scores retain original values and show a warning when weighted displayed probabilities differ by more than 0.001. Malformed distributions and out-of-range scores remain invalid. Actual OpenRouter retest passed all 51 checks. |
| Cost provenance | Valid OpenRouter `usage.cost` is recorded as provider-reported USD. Explicit configured estimates remain a fallback. Other providers' undocumented cost fields remain extensions. Parent-request accounting and import/export provenance have regression coverage. |
| Connection panel | Provider/endpoint visibility, complete Python and JavaScript snippets, identity encoding, local-model mode and first-capture feedback. |
| History navigation | Cursor pagination for requests and groups, server group search, custom date bounds and bookmarkable filters. Cursor pages do not change parent-scope totals. Backend tests traverse 235 records/groups with timestamp ties. |
| Timeline | At most 168 adaptive buckets cover the entire selected range with empty intervals and real time spacing. Dates distinguish multiday ranges. Extreme supported boundaries have regression coverage. |
| Definition comparison | Outcome distributions, review coverage, request sample sizes, failures, warnings, latency, tokens and known-cost coverage are shown with scope caveats. This is not a controlled model-accuracy evaluation. |
| Credential recovery | A replacement OS-store entry is staged before SQLite approval; a failed approval preserves the previous entry. Injected failure tests cover the transaction ordering. Downgrade may require provider-key re-registration. |
| Storage diagnostics | Collection health retains bounded, sanitized failure categories and the UI shows actionable hints. |
| Release process | Real-backend browser journey, checksum-verified published-release upgrade/rollback harness, encrypted benchmark harnesses, metadata/notes preflight and aggregate archive validation on manual runs. Prerelease tags cannot be promoted as latest. |

## Evidence

- [Laya live inference](../../reports/validation/next-release/laya-live.json): pinned English checkpoint and installed server identity, synthetic input, no provider key, encrypted temporary workspace.
- [OpenRouter candidate inference](../../reports/validation/next-release/openrouter-0.2.0.json): four successful calls, one invalid-model error and one locally rejected token. All 12 successful answers were valid; one has a consistency warning. Exact usage was 1,639 input and 284 output tokens with $0.000068838 provider-reported cost. Labels, exports, deduplication and encrypted restart passed.
- [Published 0.1.0 to candidate upgrade](../../reports/validation/next-release/upgrade-v0.2.0.json): historical records and labels, new candidate writes, rollback, backup restore, credential approval digests, wrong keys and future-schema nonmutation. This uses a downloaded baseline checked against published SHA256SUMS.
- [Compatibility guide](../compatibility.md) contains reproduction commands and exact client/model scope. [0.2.0 notes](0.2.0.md), [performance](../performance.md) and [stress](../stress.md) record final verification and operating limits.

The original evidence is retained rather than overwritten: [backend findings on 0.1.0](../../reports/validation/next-release/backend-findings.json), [failing live OpenRouter check](../../reports/validation/next-release/openrouter-live.json), [Score arithmetic](../../reports/validation/next-release/score-consistency.json), and [negative upgrade regression](../../reports/validation/next-release/upgrade-v0.1.0-regression.json). The old version accepted only 11 of 12 successful live answers, mutated unsupported plaintext history before rejecting it, and omitted aborted HTTP-200 transfers from failure totals. The candidate addresses all three. The initial upgrade harness also exposed an expected JSONL projection difference (`failed`); its corrected comparison validates that derived field while checking preserved record contents.

Local source verification passed Rust formatting, Clippy with warnings denied, **105 Rust tests** (one intentionally ignored CSV timing benchmark), **42 production-bundle browser tests** plus focused chart and local-timeout checks (44 distinct tests overall), **21 installer tests**, six release-metadata tests and locked license-notice verification. The final short-range chart correction, local-model timeout examples and both real-backend browser journeys passed their focused checks. Both pinned SDKs passed 16 mock requests with exact usage and credential exclusion; see the [candidate SDK result](../../reports/validation/next-release/sdk-0.2.0.json).

## Release boundary

Complete the final native build and verification workflow before tagging. Local checks run on Linux ARM64; macOS, Windows, other native architectures and real OS credential stores still require their platform gates. Six-platform builds and aggregate archive/checksum checks are wired into manual release runs as well as publication. These workflow changes have not themselves been exercised across all six platforms in this workspace.

The two live-provider runs are small functional checks, not performance, model-accuracy or calibration evidence. Laya's batch endpoint, extended numeric Choice labels and abstention-specific semantics are outside typed capture. Browser/provider keys and encrypted test databases are temporary; sanitized evidence contains no credentials. Preserve a stopped-workspace backup and the same database key before upgrading. See [upgrade guidance](../installation.md#history-backups-and-downgrades) for the saved-credential downgrade limitation.
