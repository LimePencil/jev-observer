# How 100 projects use Jev, and where Observer could help

Research date: September 22, 2026. This is a targeted static review of 100 public repositories, not 100 production deployments or customer interviews. Proposed benefits below are our inferences. No third-party code was executed, no inference requests were made, and no maintainers were contacted.

**Recommendation:** build a free local question-statistics tool, with both a proxy and file import. Automatically recognize recurring Jev definitions, offer inspectable families for indexed questions, and compare rule versions. Basic request logging alone is a weak differentiator because several reviewed projects already have useful logs, reports or dashboards.

The complete [100-project matrix](projects.md) records observed use, existing visibility, a specific proposed benefit and an integration constraint for every repository. The [CSV](projects.csv) supports sorting; the [manifest](manifest.json) pins sources to commits. These are research findings, not tested compatibility claims.

## What we examined

We used [awesome-jev](https://github.com/cobanov/awesome-jev/blob/a93e7281a6ca41a635eeef22b9de9b675f089231/README.md) to discover candidates and purposively selected across eight categories. The discovery extraction contained 191 distinct repository links. We excluded official SDKs, broad framework repositories, directory-only entries and open-model reproductions from this sample. Selection favors accessible implementations and varied workflows; it is not random and does not measure ecosystem prevalence.

| Discovery category | Repositories |
|---|---:|
| SDKs and developer tools | 9 |
| Agents, coding, and guardrails | 26 |
| Context and compaction | 7 |
| Browser and computer use | 9 |
| Routing, data, and workflows | 23 |
| Games, robotics, and interactive demos | 10 |
| Media and creative tools | 6 |
| Evaluation and calibration | 10 |
| **Total** | **100** |

For each repository, we collected metadata, its default-branch commit, file tree, README and selected source files. Initial filename ranking was followed by manual selection of additional request builders and policies where needed. Review consisted of README sections and targeted implementation excerpts, with focused reads for ambiguous cases. The 531 collected files are a reproducibility inventory, not a claim that every line was read. This was not a security audit or end-to-end functional test.

All 100 are distinct repositories and none was marked as a fork by GitHub. That does **not** make them independent adoption signals: several compaction and browser projects are related ports, and multiple repositories share authors. One demo monorepo counts once. Ten entries are evaluation-oriented; the Jev work in nautilus-compass is a calibration study, and jev-decision-benchmarks is primarily saved evidence and report-building code. We include those as research projects, not live application deployments. See matrix rows 36–39, 47/49, 78 and 91–100.

## Recurring question patterns

We manually assigned one primary grouping challenge per repository. Many repositories contain several patterns. Counts describe this sample and this coding scheme only; another reviewer could assign a different primary pattern. [Annotations](annotations.tsv), [derived counts](counts.json).

| Main pattern | Count | Product consequence |
|---|---:|---|
| Fixed recurring definitions | 29 | Automatic per-question charts are a straightforward starting point. |
| User/configuration-defined rules | 20 | Track definition versions and source/workflow identity. |
| Changing candidate sets | 26 | Preserve each candidate domain; global option-ID frequencies can be meaningless. |
| Indexed or contextual question instances | 18 | Exact matching fragments one workflow into many groups; use scoped template adapters. |
| Controlled study or benchmark variants | 7 | Keep experiment identity, wording and presentation differences intact. |

Thus 44 entries have changing candidates or indexed instances as their primary challenge. A design that only groups identical JSON definitions would miss a substantial part of this deliberately broad sample. That does not establish a 44% ecosystem-wide rate.

## Findings that change the product

**1. A Jev answer's shape is not its task.** A Noul used to retain tool output and one used to detect suspicious code share a primitive but need different statistics. Configured rule systems such as zod-jev and jev-pref give us useful starting identities, while different input examples should accumulate under the same rule. Keep source, rule and definition identity. [zod-jev request builder](https://github.com/jomatsu/zod-jev/blob/700bd256fe94541a2d21044027cc2dbf5036b396/src/judge.ts); see also [matrix rows 9 and 20](projects.md).

**2. Indexed instances need deliberate grouping.** jgrep uses indexed rows/chunks; compaction ports embed tool-call identifiers and lengths in questions; jev-paint embeds pixel coordinates. Treating every exact instance as a permanent group would produce fragmented or enormous dashboards. Offer a local, versioned adapter that identifies the reusable rule and the instance reference. Unrecognized variations should generate a family suggestion with a visible diff, not automatic pooled statistics. [Rows 7, 36–39, 85](projects.md).

**3. Dynamic choices need candidate context.** Browser agents, skill routers, games and tree traversal change their legal options. A label such as `a0` or `c1` has no stable meaning across those requests. Show volume, latency, candidate count and uncertainty at family level; show option frequencies only for a stable candidate set or explicit semantic mapping. Store a candidate-set fingerprint, and retain descriptions only under the user's retention settings. [Rows 26, 43–51, 64, 80–82](projects.md).

**4. Some task meaning lives in state.** The intrusion-detection prompts refer to instructions and categories carried in state. The question can remain unchanged while its effective task changes. An exact-definition chart can be useful operationally, but must not be described as proof of semantic equivalence. Support optional local metadata identifying the task/rule version or selected state paths that carry configuration. Do not hash all state into the group: that would make every input a new task. [Row 93](projects.md).

**5. Existing visibility is common enough to affect positioning.** Belay has decisions and labels; JevRouter has receipts and a dashboard; Scout has logs and live inspection; pi-warden has local traces; jevsql and jevcal already cover parts of evaluation. These examples support interoperability, not a claim that every project lacks observability. File import and question-version comparisons are stronger proposals than another isolated log viewer. [Rows 15, 24–25, 30, 69, 100](projects.md).

**6. The model answer is often only one input to the action.** Routers override uncertain choices, guards apply deterministic policy, Tetris blends distributions, and SQL or retrieval tools reuse caches. A proxy sees upstream requests; it cannot reconstruct every action or cache hit. Keep model observations, application actions and outcomes as separate linked records. Never equate a chosen option with executed work or an answer probability with measured accuracy. [Rows 16, 18, 23, 25, 52, 72, 81](projects.md).

**7. “Local” needs a clear integration boundary.** A self-hosted n8n node or a configurable local client can plausibly call a loopback collector. Hosted Apps Script, cloud workers and native provider bindings cannot automatically send their traffic through a developer's laptop. Support local imports and document reachable-runtime requirements; do not promise universal one-line setup. Native TypeSafe, OpenRouter decisions, Vercel evaluation and Cloudflare bindings are distinct compatibility work. [Rows 58, 62–63, 67, 71, 89](projects.md).

## First integrations to build and verify

These are implementation candidates, not partners. No maintainer has expressed interest in Observer.

| Candidate | Concrete first view | Why start here / remaining check |
|---|---|---|
| zod-jev, row 9 | One chart per semantic validation rule, with a definition-change comparison | Configurable client base URL and reusable rule definitions; verify wire compatibility. |
| jev-commit, row 17 | Five recurring check charts, one parent request-cost record | Small fixed batch and configurable endpoint; validate redaction and failure handling. |
| jev-belay, row 15 | Per-check trends linked to imported human labels | Existing local records make outcome integration concrete; avoid duplicating its live UI. |
| n8n-nodes-typesafe-jev, row 71 | Automatic charts per workflow and question definition | Configurable base URL; verify access from the actual self-hosted runtime. |
| jgrep, row 7 | One rule histogram across indexed rows | Use as the first indexed-family fixture; endpoint configuration may need a patch. |

A compaction adapter is a useful second fixture because the related ports expose the same fragmentation problem. Browser agents are a later integration: candidate mappings and actual action outcomes make a much richer but harder first demo. Existing evaluation suites are likely import/export users, not strong candidates for replacing their dashboards.

## Revised build order

1. Native TypeSafe proxy, request ledger, health counters and exact-definition charts. Distinguish unknown cost from zero and charge a batch once.
2. Local JSONL import into the same event model, with provenance and explicit event IDs. Add a small adapter for one existing log format.
3. One tested indexed-family adapter, initially for row filtering or compaction. Keep strict original definitions and an inspectable mapping underneath the family view.
4. Definition and presentation-version comparisons, optional task-version metadata, and candidate-aware views.
5. Link labels and application actions; provide offline threshold previews once label provenance and denominators are visible.

These remain proposed features. The survey did not produce a running proxy or prove compatibility, low overhead, cost savings or repeat use. [MVP](../mvp.md) and [grouping design](../grouping.md) contain the revised boundaries.

## How to promote a useful free project

Proposed message: **“Your recurring Jev questions, with their own local statistics.”** Show the same rule applied to many inputs, a rule edit creating a separate version, and an indexed batch appearing as one understandable family. Show cost at the request level and the selected option beside the application's actual action when available.

The first example should be a small commit-check or semantic-validation workflow drawn from the reviewed patterns. Follow it with a row-filtering example to demonstrate the feature that a raw request log does not provide automatically. Existing log import is a separate useful demo. Use synthetic inputs and clearly labeled fixture answers until live tests exist.

Release under MIT with offline sample data, no account, no subscription and no background analytics. Keep local inspection free; clarify that live Jev inference still uses the configured provider and its charges. Publish compatibility fixtures and reproducible setup instructions before making broad compatibility claims. For existing dashboard projects, offer portable question statistics or an importer rather than arguing they should replace their UI.

The research supports a concrete product hypothesis, not demonstrated demand. Next evidence should be developers connecting real workflows, answering a question their current tooling makes difficult, and choosing to use Observer again. This can be learned through volunteered feedback without adding telemetry.

## Reproduce and audit

From the repository root, with Python 3 and authenticated GitHub CLI:

```sh
# Exact saved commits and file inventory; source stays outside this repository.
python3 research/survey/scripts/replay_sources.py --cache /tmp/jev-survey-replay
# Display bounded excerpts for manual review; does not classify automatically.
python3 research/survey/scripts/review_sources.py 1 10 --cache /tmp/jev-survey-replay
# Validate 100 unique annotations and regenerate matrix, CSV and counts.
python3 research/survey/scripts/build_report.py
```

The exploratory `collect.py` ranks source paths and collects a new sample snapshot with a fresh cache; it is not the exact replay command. `supplemental-files.json` records manual follow-up file selections. `discovery.json` pins the directory used to select the sample. `manifest.json` is the authoritative complete inventory with source URLs, commits and hashes of decoded UTF-8 text. Full third-party source snapshots are not redistributed here. Fetching and static inspection do not install or execute those projects.

Validation completed: 100 unique repositories and annotations; all principal evidence paths present in their pinned manifests; 531 cached source-file hashes verified; category and pattern totals both equal 100. These checks establish consistency of this research package, not the correctness of the studied applications or of their published benchmark claims.
