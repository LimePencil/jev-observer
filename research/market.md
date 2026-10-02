# Market research

Research date: September 22, 2026. Updated to reflect the owner's decision to build free, local, open-source software. This is desk research; usability and adoption hypotheses have not been validated with users.

## Recommendation

Build a small local proxy and dashboard under the MIT license. The subsequent [100-project survey](survey/README.md) sharpens the initial value: automatic statistics for recurring Jev questions, inspectable families for indexed batches, definition-version comparisons and local log import. Request counts and cost estimates support those investigations. Several surveyed projects already have useful logs or dashboards, so basic logging alone is a weak differentiator.

Build around the investigation developers perform after an unexpected decision: inspect alternatives, identify changed questions, and connect the result to what the application actually did. Existing application events and labels are needed to assess outcomes; a proxy alone cannot infer them.

The initial product should make this question easy to answer: **“After our last change, which decisions changed, and are the changes better?”**

A decision explorer is the entry point. Local history, comparisons, labeled evaluation, and portable exports are reasons to keep using it. All features belong in the free project. Prioritize simple setup, useful defaults, and integrations that contributors can extend.

## What the evidence supports

TypeSafe announced Jev on September 15, 2026. The ecosystem is one week old at this research date. There is little history from which to infer retention or a stable market. [TypeSafe announcement](https://typesafe.ai/blog/introducing-system-one-models-and-jev)

Vercel reports that nearly 13% of its paid AI Gateway teams used Jev within its first 24 hours on the platform, more than twice the adoption of any previous launch. This is a platform-reported early adoption signal, not a count of paying customers for an observability product. [Vercel adoption report](https://vercel.com/blog/ai-gateway-jev-model-launch)

There is observable developer effort around decision visibility: JevRouter documents a local dashboard and decision receipts; jev-scout documents a decision feed and cost/call ticker; pi-jev-code records decision and execution events. Their existence suggests that these builders value visibility. It does not establish demand for another tool. [JevRouter](https://github.com/BillionsBobby/JevRouter), [jev-scout](https://github.com/kierandotai/jev-scout), [pi-jev-code](https://github.com/kamilpostrozny/pi-jev-code)

TypeSafe documents failure modes including literal interpretation, irrelevant context, contradictory criteria, and adversarial input. That makes question inspection and regression testing a credible job for the product. It does not mean a dashboard prevents errors. [Model limitations](https://docs.typesafe.ai/model-jaggedness/jev-1.13)

## Competition and substitutes

These are documented capabilities, not hands-on comparative tests. “Not verified” must never become “unsupported” in promotional copy.

| Alternative | Verified capability | Implication for this project |
|---|---|---|
| LiteLLM | Native TypeSafe pass-through, cost tracking, and logging; its page currently marks end-user tracking unsupported for this endpoint. [Docs](https://docs.litellm.ai/docs/pass_through/typesafe) | Direct competition for the original proxy idea. We need useful decision analysis beyond collection. |
| Vercel AI Gateway | TypeSafe-compatible endpoint; usage and observability; searchable request logs and exports. [Integration](https://vercel.com/docs/ai-gateway/sdks-and-apis/typesafe), [Observability](https://vercel.com/docs/ai-gateway/observability-and-spend/observability) | Gateway customers already have a default place to inspect requests. An extra network hop needs a clear benefit. |
| Langfuse | Tracing, custom dashboards, scores, evaluations, and self-hosting. [Docs](https://langfuse.com/docs/observability/overview) | A capable team can build much of the proposed workflow here. Compete on setup and investigation time; support export. |
| Helicone | Custom operation logging; free and paid observability plans. [Logger SDK](https://docs.helicone.ai/integrations/data/logger-sdk), [Pricing](https://www.helicone.ai/pricing) | Jev need not be a named integration to be loggable. General observability is not an empty market. |
| Braintrust | Custom spans can capture application inputs, outputs, errors, and metadata. [Docs](https://www.braintrust.dev/docs/instrument/trace-application-logic) | Another credible way to instrument decisions and downstream behavior. Specialized Jev displays were not verified. |
| JevRouter | Local dashboard with routing counts, latency, selected capabilities, and feedback-aware execution status. [Repository](https://github.com/BillionsBobby/JevRouter) | A decision dashboard is already being built into tools. Persistent comparisons across applications might add value. |
| jev-routing | Local proxy for coding tools with a dashboard showing rewrites and routing activity. [Repository](https://github.com/nekowasabi/jev-routing) | Do not claim to be the first Jev proxy or dashboard. |
| DIY logs / TypeSafe console | DIY can retain the API response with little code. Authenticated console functionality was not inspected. | Validate the actual console before making feature-gap claims. Small hobby projects may need nothing more. |

The hypothesis is **less work to debug typed decisions**, not exclusive access to probabilities. Competitors can store those fields and could add comparable views. A local adapter for existing observability systems may be useful alongside the standalone dashboard.

## Initial users

| Segment | Trigger to try it | Repeat job | Priority |
|---|---|---|---|
| Small teams using Jev for support/intake routing | A criteria edit sends records to the wrong queue | Compare versions, review disagreements, tune escalation | First recurring-workflow users |
| Maintainers of Jev routers and agent tools | Users report surprising choices across sessions | Inspect options, correlate decisions with actions, share an incident | First distribution and design partners |
| Retrieval and context-filtering builders | Useful context disappears | Inspect keep/drop judgments and label costly mistakes | Next segment |
| Hobbyists experimenting with Jev | Want to watch calls in a dashboard | Occasional exploration | First installation and demo users |
| Large organizations with an established tracing stack | Need decision-specific analysis within existing workflows | Export, evaluation, access control | Later; likely prefer an integration |

Following the source survey, start with semantic validation or commit checks as concrete integration candidates, then demonstrate indexed row filtering. Support routing remains an understandable alternative example with categories and reviewer corrections. Coding-tool maintainers are potential early contributors, not confirmed partners.

## Positioning that could earn continued use

1. **Decision history:** browse each question's options and result, with the request/session it belonged to.
2. **Change investigation:** distinguish changed question definitions, model versions, policy versions, and traffic cohorts.
3. **Threshold evaluation:** compare automation coverage and observed errors using labeled examples.
4. **Outcome linkage:** show separately what Jev selected, what the application executed, and what a reviewer marked correct.

Choice and Score include distributions and a confidence statistic. Noul has a probability and no separate confidence field. Confidence is not empirical accuracy. Calibration views require labels and appropriate probabilities; missing outcomes must remain unknown. [TypeSafe confidence documentation](https://docs.typesafe.ai/confidence)

Proportions shifting between Tuesday and Wednesday do not prove a regression. Traffic may have changed. Paired evaluation on the same examples provides stronger evidence, while historical cohort comparisons should be labeled descriptive.

## User costs and local resource use

The documented direct TypeSafe rate is $0.042 per million input tokens, with output free. These examples are arithmetic at that rate, not invoices or benchmarks. [TypeSafe models and pricing](https://docs.typesafe.ai/models)

| Monthly requests | Mean input tokens per request | Estimated inference spend |
|---:|---:|---:|
| 10,000 | 1,000 | $0.42 |
| 100,000 | 1,000 | $4.20 |
| 1,000,000 | 1,000 | $42.00 |
| 100,000 | 5,000 | $21.00 |

Formula: requests × input tokens ÷ 1,000,000 × $0.042. A request can answer several questions, so request count and decision count are different. Shared input prevents exact per-question cost attribution from aggregate token usage alone.

Jev Observer is free software, with no subscription, inference markup, or paid feature gates. Users bring their own provider credentials and pay any provider charges directly. The dashboard should make those external costs clear. Sample data, local history browsing, and offline threshold previews should work without API access.

Local storage can become significant even when inference is inexpensive. Illustratively, one million stored request bundles averaging 2 KB occupy 2 GB before indexes, replication, and backups. The 2 KB assumption is unmeasured and may be much larger with many questions/options. Measure bytes per request and per answer. Provide user-controlled retention, deletion, export, and a visible disk-usage estimate; document how batched questions affect storage. Resource limits protect the local machine and are not paid-plan restrictions.

## Main uncertainties and ways to resolve them

| Uncertainty | Evidence needed |
|---|---|
| Is the pain recurring? | Five users show a recent debugging incident and what they did to solve it. |
| Is it better than the tools they have? | Users complete the same investigation using their current logs and our explorer; record time and mistakes. |
| Will they adopt another collector? | Observe setup, then measure continued ingestion and return visits. Test local and later asynchronous ingestion if proxying is a blocker. |
| Will they keep using it? | Return use on a real workflow after the initial demo, with a concrete useful investigation. |
| Is it lightweight enough? | Measure local memory, event size, disk growth, and collector overhead under representative workloads. |
| Is the niche durable? | Track repeat use after the launch excitement and reassess whether a broader decision-analysis adapter is warranted. |

No interviews, deployments, community posts, or outbound messages were conducted during this research. Search coverage is not exhaustive; no claim of market uniqueness is justified.
