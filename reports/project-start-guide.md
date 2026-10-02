# Jev Observer
## A practical guide to starting the project

Free local software for understanding recurring Jev decisions.

**Product recommendation:** build a local proxy and log importer that recognizes recurring Jev questions, creates useful statistics, and makes definition changes visible.

**Evidence base:** a targeted static review of 100 public repositories, with an individual benefit hypothesis and integration constraint for every project.

**Prepared:** September 22, 2026

**Current status:** research and design. No application has been implemented, no compatibility or performance measurements have been completed, and no user interviews have been conducted.

**Project:** [LimePencil/jev-observer](https://github.com/LimePencil/jev-observer) — private during development, with an MIT license prepared for an eventual open-source release. Jev Observer is a working name and an independent project.

Local history, charts, labels and exports should be free. Live Jev inference still goes to the configured external provider and can incur its charges. Grouping should require no additional model calls.

<!-- page -->
# 01 / The decision to make

**Build a Jev-specific question-statistics tool.** Start with recurring validation or commit-check questions, then support one indexed-batch pattern. A broad proxy with a spend dashboard is less distinctive: several reviewed tools already have logs and dashboards.

The first promise should be: **“Your recurring Jev questions, with their own local statistics.”** A user points a compatible local client at Observer, or imports a supported log, and gets a meaningful view without configuring every chart.

## Decisions already established

| Decision | Working direction |
|---|---|
| Product | Jev-specific; Choice, Score and Noul awareness |
| Deployment | Local executable, browser UI, local database |
| Price and license | Free, MIT, no paid feature tiers |
| Data | No background analytics or automatic uploads |
| Repository | Private development now; public release is a separate future action |
| Initial collection | Native TypeSafe proxy plus local file import |
| First differentiation | Recurring question statistics and inspectable version changes |

## Reading map

Pages 3–5 explain the evidence, users and first experience. Pages 6–10 define grouping, statistics, architecture, privacy and costs. Pages 11–14 cover implementation, verification, launch and unresolved decisions. Page 15 links primary sources. Pages 16–40 contain all 100 project assessments, four per page.

## What this report does not establish

Repository activity is not proof of production use, willingness to adopt another tool, or sustained demand. The reviewed sample includes related ports and evaluation projects. Proposed integrations, benefits and milestones still need hands-on validation. No outreach or publication has been performed.

<!-- page -->
# 02 / What Jev and the evidence tell us

Jev accepts input state and named questions through TypeSafe's system-one API. Its typed outputs create useful chart defaults: Choice selects among defined options; Score evaluates an ordered rubric; Noul reports a yes-probability. Instructions may themselves be structured. Responses associate answers with question keys and report usage for the request. [API reference](https://docs.typesafe.ai/api).

## Main grouping challenge in each sampled repository

[PATTERN_CHART]

The categories are exclusive manual primary assignments, not a complete inventory of every question. Fixed definitions and configurable rules account for 49 entries. Indexed instances and changing candidates account for 44. Seven primarily focus on controlled question or benchmark variants. These are sample counts, not ecosystem-wide percentages.

## How the review was conducted

We selected 100 distinct repositories across eight categories from awesome-jev, inspected README sections and targeted implementation excerpts, and collected 531 source files at pinned commits. Supplemental request builders were examined when filename ranking missed the relevant implementation. Collected files are not a claim of exhaustive line-by-line review.

The sample has no GitHub-declared forks, but contains related ports and shared authors. Ten entries are evaluation-oriented, including a report/artifact study. The main nautilus-compass application is not counted as an established Jev runtime integration; its included work is a calibration study. No studied project or benchmark was executed. The appendix preserves these distinctions.

<!-- page -->
# 03 / Who should use it first

Prioritize developers who repeat the same semantic rule and change it over time. They offer a clearer first investigation than a complex autonomous browser loop.

| Candidate | Useful first result | Main integration check |
|---|---|---|
| zod-jev — #9 | Per-rule validation distributions and version changes | Test configurable base URL and issue mapping |
| jev-commit — #17 | Five recurring checks, one parent cost record | Test transport and sensitive-source retention |
| jev-belay — #15 | Check trends linked to existing labels | Import logs; complement its current live view |
| n8n node — #71 | Charts per workflow and question | Verify self-hosted runtime can reach collector |
| jgrep — #7 | One histogram across indexed rows | Add/test endpoint setting and family mapping |

Numbers refer to the project appendix. These are candidates inferred from source review, not partners or confirmed users. Browser agents and compaction tools are valuable later fixtures because their identifiers and candidate sets change frequently.

## Why a developer would keep it

They can answer a recurring question: “Did my rule change, did the inputs change, or did the model change?” The explorer should show the definition, alternatives, version and context needed to investigate. Measured correctness requires labels; comparing two dates alone does not prove regression.

## Existing alternatives

Gateway logs, general observability platforms and application-specific dashboards already solve parts of this problem. LiteLLM documents TypeSafe pass-through; Vercel offers gateway observability; Langfuse offers tracing and evaluation. JevRouter, Scout and Belay already expose useful decisions. Sources are linked on page 15 and in appendix entries.

Compete on Jev-specific defaults and fewer setup steps. Offer imports and exports. Do not claim to be the first Jev proxy, to have exclusive access to probabilities, or to replace every existing dashboard. Validate whether Observer makes a concrete investigation easier.

<!-- page -->
# 04 / The first useful experience

## A short user journey

1. Start Observer locally and open its bundled browser UI. Sample mode works without a provider key or internet access.
2. Connect a supported local client, or import a supported local JSONL file. Show the source name, collection status and any errors.
3. Send several inputs through the same named Jev rules. Observer creates recurring groups and suitable charts.
4. Open an unusual answer. Inspect its question definition, available distribution, parent request and cost basis.
5. Edit a rule. The next call appears under a separate definition version, with a visible comparison.
6. Attach a correction or import an application outcome. Keep this evidence distinct from the model's own answer.

## Essential screens

| View | What the user needs to see |
|---|---|
| Overview | Source, time window, requests, answer observations, errors, usage, cost basis, collector health |
| Question group | Definition, version, sample count, appropriate histogram or frequencies, model filter |
| Decision detail | Original question key, answer, alternatives, candidate context, linked action and label if supplied |
| Changes | Definition diff, presentation/task version, separate series and matched-example comparison when available |
| Data controls | Retention, payload capture, import/export, deletion, disk use |

## First release boundary

Include a native proxy, local history, automatic definition charts, file import/export, one indexed-family adapter and a synthetic demo. Add basic labels and richer comparisons next. Keep future improvements in the same free application.

Defer universal provider support, model routing, hosted accounts, collaboration servers, automatic prompt optimization and generic structured-output detection. These add scope without proving the core question-statistics workflow.

<!-- page -->
# 05 / How automatic grouping should work

**Use the request definition, not just the answer shape.** Two Noul outputs can represent entirely unrelated tasks. Recognition must be local, deterministic and inspectable.

## Keep two levels of identity

**Definition series:** source namespace + original question key + complete definition fingerprint + presentation version + optional task-version discriminator. Preserve instructions, criteria, extra fields, strings and array order. A model version is a visible filter rather than a reason to discard history.

**Question family:** a reusable operation linking compatible instances or versions. Keep the original series underneath it. A family relationship is not permission to pool every statistic.

## Important cases

| Case | Required behavior |
|---|---|
| Same rule, new input | Accumulate answer observations under the recurring definition |
| Instructions or rubric changed | New definition version; show a diff |
| Presentation/order changed | Preserve a presentation version and separate default series |
| Renamed key | Suggest a relationship; keep original identity |
| Row/tool/pixel index changed | Use a tested source-specific adapter; retain instance reference |
| Candidate options changed | Preserve candidate-set fingerprint; scope label frequencies |
| Rules embedded in state | Use optional task-version metadata or selected configured state paths |

## Limits of “automatic”

If a question refers to rules in state, identical question text does not prove identical task meaning. Without a discriminator, label the group definition-based with unverified task-version context. Never hash all input state into identity: that makes every example a new task.

For unknown indexed patterns, offer a family suggestion and visible structural diff. Never remove all numbers or identifiers blindly: a changed age limit or threshold could be the rule itself. Version adapters and mapping reasons. Bound discovery work so a pixel workload cannot create thousands of visible charts.

<!-- page -->
# 06 / Statistics that mean what they say

| Question | Default view | Interpretation boundary |
|---|---|---|
| Choice | Selected-option frequency and reported probability distributions | Only combine stable option meanings; candidate IDs may be request-local |
| Score | Histogram, percentiles and level distributions | Compare within the same rubric; a score is not an accuracy percentage |
| Noul | Yes-probability histogram and trend | Show threshold crossings only when the threshold is supplied |
| All | Answer count, distinct request count, associated latency and cost | Parent metrics overlap across groups; they are not exact per-question billing |

Choice and Score confidence summarizes their probability distributions. Noul has no separate native confidence field. Preserve any application-derived metric with its origin; do not relabel it as a native field. [Confidence documentation](https://docs.typesafe.ai/confidence).

## Four records users must not confuse

1. **Model answer:** what Jev returned.
2. **Policy decision:** what the application selected after thresholds, overrides or combined scores.
3. **Execution result:** whether that action ran and what happened.
4. **Outcome label:** a human or process judgment of correctness or usefulness, with provenance.

A proxy directly observes the first. The other records require imports or application events. Cache hits, deterministic fast paths and heuristic decisions may never reach the proxy.

## Denominators and comparisons

Twenty indexed questions in one request produce twenty answer observations and one parent request. A histogram weighted by answers differs from one weighted by requests; label the choice. Show missing and invalid answers separately from valid observations.

Keep timestamps, model/version, definition version, cohort and sample count visible. Distribution changes are descriptive unless compared on matched retained inputs. Labels from a teacher model are not independent ground truth. Reviewed samples can be biased; report label coverage and use a held-out representative set before recommending a policy threshold.

Unknown cost, missing labels, timeouts and absent execution outcomes must stay unknown.

<!-- page -->
# 07 / Proposed architecture

Use a small Rust service, SQLite and a bundled browser interface. This is a proposed implementation choice for a simple local package, not a benchmarked performance claim. Select and pin specific libraries when implementation starts.

[ARCHITECTURE]

## Separate responsibilities

- **Forwarder:** accept the supported Jev route, forward to a configured upstream, preserve responses and transport outcomes.
- **Collector:** emit bounded request and answer events; expose queue drops, write failures and last successful capture.
- **Normalizer:** retain provider provenance, parse supported fields, preserve unknown fields or indicate when capture is incomplete.
- **Grouping engine:** compute definitions and families, apply versioned adapters and produce explainable mappings.
- **Store and importer:** validate events, migrate the local schema and preserve explicit source IDs.
- **Explorer:** query local aggregates and records without exposing provider credentials.

## Failure behavior

Telemetry failure should be visible while forwarding continues according to documented behavior. A stopped proxy process still prevents traffic through it; asynchronous ingestion or file import is an alternative for applications that cannot accept that dependency.

Do not introduce an invisible extra retry layer. Record attempts separately and correlate them only with explicit identifiers. Preserve upstream errors; do not transform them into negative answers. Bound inspected/captured payload sizes and mark truncation. Exact limits need workload measurements.

Start with native TypeSafe HTTP. OpenRouter decisions, Vercel native evaluation and Cloudflare bindings need separate fixtures and normalization. Hosted runtimes cannot automatically reach a laptop's loopback listener.

<!-- page -->
# 08 / Data, privacy and local operations

## Minimum local records

| Record | Essential fields |
|---|---|
| Request | Local ID, source, timestamps, provider/model, status, usage, reported or estimated cost and rate version |
| Answer | Request ID, original key, type, definition/presentation IDs, value, available probabilities/confidence |
| Family mapping | Family and adapter versions, task/candidate discriminator, instance reference, reason |
| Action/outcome | Linked answer/request, policy version, attempted action, result or label, source and timestamp |
| Import | Import/source IDs, original event ID when available, original timestamp, format and provenance |
| Health | Dropped events, write failures, queue pressure, capture/redaction indicators |

Compute account totals from the request ledger. Import deduplication requires explicit identity, not identical content: a second real call can be identical to the first. Keep database migrations versioned and export formats documented.

## Retention and security defaults

- Bind to loopback. Use a configured upstream allowlist; avoid an arbitrary-URL forwarding endpoint.
- Keep provider keys out of the database, browser responses and exported records. Separate forwarding credentials from local UI access.
- Persist no raw input state by default. Make payload capture an explicit local choice with redaction and retention settings.
- Apply controls to question text, criteria and labels too. Hashes are useful identifiers, not anonymization guarantees.
- Bundle UI assets and disable automatic analytics/uploads. Defend local mutation endpoints against unwanted browser-origin requests.
- Provide local export, deletion and backups. Explain that deletion in the active database does not remove separate backups or user exports.

A record without retained input cannot support exact replay. Show that limitation. Re-evaluation is a new provider call; changing a threshold over already saved answers is an offline calculation. Review every outbound request in the release verification.

<!-- page -->
# 09 / What “free and local” costs

Observer should have no subscription, inference markup or feature gates. Local exploration, sample mode, export and offline calculations require no paid service. Users remain responsible for the external inference they choose to run.

## Dated provider example

On September 22, 2026, TypeSafe lists Jev 1.13 at **$0.042 per million input tokens**, with output tokens free. This is the direct provider's published rate, not a gateway-wide promise or invoice amount. Verify the applicable model/provider rate before a live run. [Model and pricing reference](https://docs.typesafe.ai/models).

| Requests | Mean input tokens | Illustrative inference spend |
|---|---:|---:|
| 10,000 | 1,000 | $0.42 |
| 100,000 | 1,000 | $4.20 |
| 1,000,000 | 1,000 | $42.00 |
| 100,000 | 5,000 | $21.00 |

Formula: requests × mean input tokens ÷ 1,000,000 × rate. These examples exclude gateway-specific charges or discounts and assume usage is known. A multi-question request must not multiply its bill by the number of answers.

## Local resource budget

Measure bytes per request and per answer, including candidate distributions and database indexes. An illustrative million bundles averaging 2 KB occupy about 2 GB before indexes and backups; 2 KB is an unmeasured assumption and may be far too small for large batches.

Use configurable retention, disk-usage reporting and bounded queues. Measure memory, latency and storage growth on fixed-question, indexed-batch and large-candidate fixtures. Publish hardware, sample size and workload with results. Do not advertise “no overhead” before measuring it.

## Maintainer costs

The main commitment is engineering and maintenance: compatibility fixtures, packaging, migration tests and user support. A domain or hosted demo is optional. There is no proposed revenue target. Keep recurring infrastructure optional so users can build, inspect and run the software independently.

<!-- page -->
# 10 / Build in reviewable milestones

This is a sequence with exit criteria, not a delivery-date promise. Finish each thin slice before expanding provider or application coverage.

| Milestone | Deliverable | Exit evidence |
|---|---|---|
| 1. Fixtures and storage | Synthetic Choice/Score/Noul batches, event schema, local history and sample UI | Multiple answers reconcile to one request; offline browsing works |
| 2. Native forwarding | TypeSafe pass-through, redaction and visible health | Success/errors preserved; secrets absent; pinned clients exercised |
| 3. Recurring charts | Definition identity, basic distributions, presentation/version views | Stable rules accumulate; changed rules separate |
| 4. Import and family | One existing log adapter and one indexed-batch adapter | Explicit IDs deduplicate; mapping is inspectable; request cost remains unique |
| 5. Corrections and comparison | Labels, action links and version comparison | Unknowns visible; label provenance preserved; same-input comparison demonstrated |
| 6. Release candidate | Build instructions, supported binaries, checksums and demo | Clean install, offline operation and documented limits verified |

## Suggested module boundaries

Keep forwarding, event storage, grouping and provider adapters separable. Put fixture data and adapter expectations beside the relevant modules. Bundle the UI only after its local data contract is stable. Prefer one executable and one local store over an initial service cluster.

## First work session

Choose the first supported runtime and one integration fixture. Define the request/answer schema and a synthetic mixed batch. Implement storing and inspecting that batch offline before wiring live credentials. Then add native forwarding and exercise successful responses, errors and a storage failure.

Use the existing research repository and preserve its evidence. Pin dependencies and SDK versions in the implementation. Do not convert a source review into a compatibility claim until a real fixture has passed.

<!-- page -->
# 11 / What must be verified

## Correctness and accounting

- One request with three questions creates one usage/cost record and three linked answer observations.
- Valid answers, missing answers, invalid bodies and provider failures have distinct states.
- Definition and presentation changes remain inspectable; shared shapes do not merge unrelated tasks.
- A recognized indexed batch creates one family while retaining all original definitions and instance references.
- Dynamic candidate IDs never become stable labels without a semantic mapping.
- Task-version metadata separates state-carried rule changes when supplied; absent context stays unverified.
- Reimporting the same explicit source event does not duplicate it; repeating a real call still records another observation.

## Transport and operational behavior

Test success and error forwarding, timeouts, client retries, unknown fields, queue overflow, database write failure and process restart. Check that collection gaps are visible. Measure direct versus proxied behavior with representative payload sizes and concurrency before setting performance targets.

Test imports with malformed rows, unsupported formats, missing fields and duplicate IDs. Preserve a useful error report without exposing credentials or raw sensitive content. A failed row must not silently become a valid zero-valued observation.

## Local trust and usability

Verify no credentials reach stored events, UI responses or exports. With networking disabled, sample mode, saved-history browsing and exports must still work. Inspect outbound connections for remote assets, analytics and account checks. Check retention and backup/export behavior.

Ask a developer to connect a supported workflow from a clean setup, find one unusual answer, understand its alternatives, and explain a changed rule. Record blockers through volunteered feedback. A proposed learning goal is four of five observed users reaching a useful decision within ten minutes; this is a target, not an achieved benchmark.

## Release gate

Ship supported combinations with pinned versions, known limitations and reproducible examples. Mark everything else experimental or unverified. Passing local fixtures is necessary but does not establish accuracy, production reliability or broad SDK support.

<!-- page -->
# 12 / A launch that demonstrates usefulness

## The message

**Your recurring Jev questions, with their own local statistics.** Show how quickly the tool answers a real question. “See what Jev chose. Catch what changed.” is a supporting message.

## The first 90-second demonstration

1. Run a small synthetic validation or commit-check workflow.
2. Show named question groups appearing automatically across different inputs.
3. Inspect one answer and its parent request, usage and cost basis.
4. Edit a rule and compare its separate definition versions.
5. Show a labeled correction, then distinguish a saved-answer threshold preview from a new live evaluation.
6. Show an indexed batch: twenty answer observations, one recognizable family, one request cost.

Clearly label synthetic or recorded output. Publish live-measurement details only when live tests exist. Do not imply user endorsements or claim accuracy improvements from an attractive chart.

## Distribution sequence

Prepare an MIT public release with local demo data, build instructions and supported binaries. Offer integration examples to relevant maintainers when outreach is separately authorized. Then consider Jev directories/community channels and a technical article about indexed grouping or request-cost accounting. A broad launch is more useful after developers can run the tool themselves.

No messages, submissions or promotions have been sent. The repository remains private. Its future public release is not accomplished by placing an MIT file in a private repository.

## Evidence of adoption

Seek concrete debugging incidents, observed setup sessions and useful return sessions. Ask what the developer learned that their existing logs made difficult. Compare investigation steps, not just stated enthusiasm. Use voluntary feedback and public contribution activity; do not add background usage analytics.

If users only need basic statistics, keep the project small. If proxy setup repeatedly blocks adoption, prioritize import or asynchronous ingestion. If existing dashboards already answer the question, reconsider the proposed integration rather than manufacturing a gap.

<!-- page -->
# 13 / Risks and decisions still open

| Risk | Response and next evidence |
|---|---|
| Grouping combines unrelated tasks | Strict definitions, versioned adapters, candidate context and optional task metadata |
| Similar instances fragment history | One indexed-family fixture; bounded suggestions for unknown formats |
| Existing dashboards are sufficient | Observe a real investigation; prioritize imports or narrower views |
| Local proxy cannot reach the workload | Verify runtime placement; offer local file import |
| Proxy adds failure or latency | Bound capture, expose health, measure overhead; document process dependency |
| Stored definitions leak sensitive content | Include questions, criteria and labels in retention/redaction controls |
| Cost or accuracy charts mislead | Preserve provenance, denominators, unknown values and label coverage |
| Upstream APIs or aliases change | Pin compatibility fixtures and record requested/returned model IDs |
| Early ecosystem attention fades | Seek repeat use before expanding scope |

## Resolve during implementation

Choose the first supported operating system and packaging target. Pick the Rust HTTP, database and UI libraries after checking their current releases and licensing. Set retention defaults and payload limits from measured fixtures. Select the first importer and its exact supported log version. Decide how users supply optional task-version metadata without modifying upstream API bodies.

The working name remains provisional. No naming clearance or TypeSafe affiliation has been established. Public release timing remains a separate decision.

## Start here

1. Use zod-jev or jev-commit as the first compatibility candidate.
2. Build the offline mixed-batch explorer and correct request-level accounting.
3. Add native forwarding, then recurring charts.
4. Import one existing local log and normalize one indexed family.
5. Observe whether a developer can complete a useful investigation.

The next milestone is a running, verified local slice with the bounded scope described here.

<!-- page -->
# 14 / Sources and reproducibility

## Official technical references

- [TypeSafe API](https://docs.typesafe.ai/api): request/answer contract and usage. Rechecked September 22, 2026.
- [TypeSafe models](https://docs.typesafe.ai/models): dated direct-provider pricing and model versions. Rechecked September 22, 2026.
- [TypeSafe confidence](https://docs.typesafe.ai/confidence): native uncertainty fields and their interpretation. Rechecked September 22, 2026.
- [TypeSafe Python client](https://docs.typesafe.ai/sdk/python/api/clients/sync): configurable client base URL.
- [Vercel TypeSafe API](https://vercel.com/docs/ai-gateway/sdks-and-apis/typesafe): one compatible gateway route; native evaluation is separate work.

## Alternatives and discovery

- [LiteLLM TypeSafe pass-through](https://docs.litellm.ai/docs/pass_through/typesafe), [Vercel observability](https://vercel.com/docs/ai-gateway/observability-and-spend/observability), [Langfuse observability](https://langfuse.com/docs/observability/overview): documented substitutes, not hands-on comparative results.
- [awesome-jev discovery snapshot](https://github.com/cobanov/awesome-jev/blob/a93e7281a6ca41a635eeef22b9de9b675f089231/README.md): source directory for the purposive sample.

## Research package in the repository

The research/survey folder contains the readable findings, 100-row matrix, CSV, manual annotations, sample list, source manifest, discovery record and reproduction scripts. Source links in the appendix are pinned to the reviewed commits. Full third-party source snapshots remain outside this repository.

The build_report.py script validates 100 unique annotations and their evidence paths, then generates the matrix and counts. replay_sources.py retrieves the exact manifest files and verifies decoded-text hashes without executing studied projects. All 531 cached source hashes were checked; three fresh downloads were also verified.

The PDF is generated from this guide and the same survey annotations. Rebuild instructions and dependency versions are in reports/README.md. The accompanying Markdown research is editable; this PDF is a dated snapshot. Primary-source links may require internet access, and the private project repository requires permission to view.

## How to read the appendix

Each card separates observed use, existing visibility, proposed help and integration limits. “Observed” means static source/documentation evidence, not successful execution. Principal implementation and README links appear on every card; additional collected files are listed in the manifest. One primary grouping pattern is assigned per repository, even when it uses several.
