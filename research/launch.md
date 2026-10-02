# Positioning and launch plan

This is historical draft research for a free, local, MIT-licensed project. Features must be built and verified before presenting them as available; use the current README for shipped behavior. Channel choices and adoption targets are hypotheses, and no outreach is implied by this document.

## The message

The [100-project survey](survey/README.md) supports a more concrete feature message: **“Your recurring Jev questions, with their own local statistics.”** This is still a demand hypothesis. Lead with semantic validation or commit checks, then demonstrate indexed row filtering. Offer local import to projects that already have decision logs; several also have useful dashboards.

**Headline:** See what Jev chose. Catch what changed.

**Supporting copy:** A free, open-source dashboard that recognizes your recurring Jev questions and builds their statistics automatically. Inspect choices, probabilities, latency, and estimated cost on your machine, with local history and question-version comparisons.

**Short description:** A free local decision explorer for Jev.

**Initial call to action:** Try the local demo.

**Contributor call to action:** Explore the source and contribute an integration.

No account or subscription is planned. Explain that live Jev requests still use the configured external provider and its billing; the dashboard itself is free. The sample demo should run offline.

Lead with a concrete incident: “A question edit changed where your tickets go. Find the affected decisions and review the alternatives.” Statistics and spend reinforce the story, but should not carry the entire pitch.

The working name is provisional. Describe compatibility with Jev and state independence from TypeSafe. No affiliation or naming clearance has been established.

## Message tests

| Message | Audience | Expected action | Evidence to collect |
|---|---|---|---|
| Find the decisions your last update changed | Teams editing routing criteria | Connect an existing workflow | They inspect a changed decision from their own data |
| Inspect the options behind every Jev choice | Router/tool maintainers | Try a local collector | They use it to investigate a real report |
| Test thresholds against labeled outcomes | Teams choosing automation/review boundaries | Import a labeled dataset | They compare two policies and explain the tradeoff |

Use the same demo and CTA while changing one message at a time. Use volunteered feedback and aggregate public download information where available; do not add remote usage analytics or track individual installations. Local decision logs remain part of the application's core functionality. Small samples support qualitative learning, not declarations of statistically significant conversion gains.

## The first demonstration

Build a 60–90 second walkthrough of semantic validation or commit checks, using the surveyed zod-jev and jev-commit patterns as implementation candidates. Use synthetic inputs and clearly identify simulated outputs until a real run exists.

1. Show a request containing several named semantic checks.
   Send several different inputs through the same definitions and show the dashboard automatically creating separate question groups and updating their charts, without per-call tags.
2. Open one decision: definition, answer, available probability information and the application's threshold when supplied.
3. Compare two saved question versions on the same small set of inputs.
4. Open an input whose verdict changed. Show the changed criteria and reviewer label side by side.
5. Adjust a threshold in an offline policy preview. Show how many labeled examples would be automated or reviewed and the observed errors among automated examples.
6. Show request-level estimated spend and the distinction between several answers and one billed request.

The feature demonstration needs no dramatic accuracy claim. Once live measurements exist, publish dataset construction, labels, model/version, sample size, failures, token counts, and cost basis. A small synthetic dataset demonstrates the UI; it does not establish production accuracy.

Follow with a short indexed-batch example: twenty rows, one recurring rule, twenty answer observations, one request-cost record. Show the adapter's mapping so users can inspect why instances belong together. Do not promise generic semantic equivalence detection or plug-and-play interception of cloud-hosted applications.

“Replay” must mean a new evaluation with a visible cost estimate. Changing a threshold over existing answers is an offline calculation. Keep those actions visibly distinct.

## Distribution sequence

| Channel | Useful contribution | Why it fits | Status |
|---|---|---|---|
| GitHub public release | MIT license, source, local demo, release binaries, setup and contribution guides | Lets users inspect, run, modify, and share the tool | Private development now; public release later |
| Existing Jev project maintainers | A reproducible integration example and a decision-history view for their workflow | They already understand the API and several build dashboards | Candidate partners; not contacted |
| TypeSafe community | A short debugging walkthrough and a question about current pain | Concentrated group of potential early users; docs link to its community | Planned, subject to community rules |
| Jev project directories | Submit a working project with a clear compatibility statement | Relevant discovery; listing is not guaranteed | Planned after working release |
| Technical articles/search | Explain request vs decision cost, changed criteria, and labeled threshold evaluation | Answers specific integration questions and demonstrates the product | Draft topics |
| Show HN / broader developer launch | A working demo plus an honest account of findings | Appropriate after first users can run and assess it | Later |

Examples worth researching with maintainers: [JevRouter](https://github.com/BillionsBobby/JevRouter), [jev-scout](https://github.com/kierandotai/jev-scout), and [pi-jev-code](https://github.com/kamilpostrozny/pi-jev-code). [awesome-jev](https://github.com/cobanov/awesome-jev) is a relevant directory. These are distribution candidates, not endorsements, customers, or verified unmet needs.

Avoid a broad paid-ad campaign initially. The most useful early asset is a working example that saves a developer a debugging step. Search-result pages about Jev are crowded with independent guides; another general “What is Jev?” page has little distinctive value.

Suggested article titles:

- “Track each Jev answer without counting request cost twice.”
- “What changed when we edited a Jev routing question?”
- “Evaluate a Jev threshold using labeled outcomes.”

Use a factual comparison page once integrations have been exercised: “When to use your gateway logs, and when a decision explorer helps.” Do not assert that Langfuse, LiteLLM, or Vercel cannot capture decisions.

## Discovery before a public launch

Seek 8–12 conversations across recurring classification users and tool maintainers. This is a proposed next activity, not completed research.

Ask users to show, rather than speculate:

1. The last surprising decision, its impact, and the logs they inspected.
2. How frequently they change instructions, criteria, model IDs, or thresholds.
3. How they know whether a selected action was executed and whether it was correct.
4. What their current dashboard already does well.
5. Whether they can share redacted events or run a collector locally.
6. What would make them keep using it, recommend it, or contribute an integration.

Capture role, workflow frequency, recent incident, current workaround, integration blocker, useful feature, and next commitment. Do not collect credentials or raw customer content as interview notes.

Draft invitation for later manual use:

> I’m building a free local open-source tool for inspecting Jev choices and comparing changes to questions. I saw that your project uses Jev. How do you investigate a decision that looks wrong today? If that is a recurring problem, I can share a local demo using sample data.

Do not send this automatically or post unsolicited promotions in project issues. Distribution execution is separate from this research deliverable.

## Open-source release plan

Publish the source with its MIT license when the repository is made public. Keep history, comparisons, outcome labels, exports, and future improvements in the free project. There are no pricing experiments or commercial feature tiers.

For the first runnable release, provide source-build instructions, versioned binaries for supported platforms, checksums, and an offline sample dataset. Document where local data lives, how to delete or export it, and which requests leave the machine. Clearly separate provider charges from this tool's zero price.

Add a contribution guide with real build/test commands once implementation exists, plus a small set of reproducible integration tasks. Evaluate project health through voluntarily reported repeat use, resolved setup problems, and useful contributions. Stars and downloads are discovery signals, not proof that the tool solves a problem.

## Two-week validation sequence

This is a proposed sequence after research, not a promised delivery schedule.

| Period | Work | Decision evidence |
|---|---|---|
| Days 1–3 | Discovery conversations; inspect users' existing logs | At least five concrete incidents/workarounds, not just enthusiasm |
| Days 4–7 | Local collector, decision view, synthetic demo; observe five setup sessions | Four of five see their first useful decision within ten minutes without code surgery |
| Days 8–10 | Add version comparison and simple outcome labels | At least three users complete a real investigation; document where the tool helped or failed |
| Days 11–14 | Return-use check, installation docs, and contribution feedback | Three users report a useful second session; document integration requests and installation blockers |

These are deliberately small learning goals, not industry benchmarks. Track activation as “first decision inspected from the user's own workflow,” not account creation. Track retained use as a useful second session, not an open browser tab.

Prioritize improvements that make local investigations easier and integration simpler. Collect feedback through opt-in conversations and user-submitted issues, without background analytics. If users only need basic statistics, keep the project small and useful; there is no revenue target or hosted-service expansion gate.
