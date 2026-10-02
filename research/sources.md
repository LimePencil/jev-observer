# Source register

Checked September 22, 2026. Primary documentation and project-authored descriptions support the technical claims. Vendor adoption and pricing statements are attributed; none were independently benchmarked. Repository documentation is evidence of advertised functionality, not a production-quality assessment.

| Source | What it establishes |
|---|---|
| [TypeSafe launch announcement](https://typesafe.ai/blog/introducing-system-one-models-and-jev) | Launch date and vendor framing |
| [TypeSafe introduction](https://docs.typesafe.ai/introduction) | Typed question/answer model |
| [TypeSafe API](https://docs.typesafe.ai/api) | Request, response, usage, and error contracts |
| [TypeSafe model reference](https://docs.typesafe.ai/models) | Published rates and model aliases |
| [TypeSafe confidence](https://docs.typesafe.ai/confidence) | Confidence semantics and Noul distinction |
| [Jev 1.13 limitations](https://docs.typesafe.ai/model-jaggedness/jev-1.13) | Documented failure modes |
| [TypeSafe Python client](https://docs.typesafe.ai/sdk/python/api/clients/sync) | Configurable API root |
| [TypeSafe SDK overview](https://docs.typesafe.ai/sdk) | Default retry behavior |
| [Vercel Jev adoption report](https://vercel.com/blog/ai-gateway-jev-model-launch) | Vendor-reported early usage signal |
| [Vercel TypeSafe API](https://vercel.com/docs/ai-gateway/sdks-and-apis/typesafe) | Compatible endpoint, SDK configuration, and gateway metadata |
| [Vercel observability](https://vercel.com/docs/ai-gateway/observability-and-spend/observability) | Existing logs and monitoring |
| [LiteLLM TypeSafe pass-through](https://docs.litellm.ai/docs/pass_through/typesafe) | Direct proxy competitor and accounting capabilities |
| [Langfuse observability](https://langfuse.com/docs/observability/overview) | Existing tracing, scores, dashboards, and evaluation |
| [Helicone custom logging](https://docs.helicone.ai/integrations/data/logger-sdk) | Generic operation instrumentation |
| [Helicone pricing](https://www.helicone.ai/pricing) | Reference for an alternative product’s free/paid offering |
| [Braintrust custom tracing](https://www.braintrust.dev/docs/instrument/trace-application-logic) | Application-level tracing substitute |
| [JevRouter](https://github.com/BillionsBobby/JevRouter) | Decision receipts, dashboard, and execution-feedback distinction |
| [jev-routing](https://github.com/nekowasabi/jev-routing) | Existing local proxy/dashboard |
| [jev-scout](https://github.com/kierandotai/jev-scout) | Workflow-specific decision visibility |
| [pi-jev-code](https://github.com/kamilpostrozny/pi-jev-code) | Decision/action telemetry in a builder project |
| [awesome-jev](https://github.com/cobanov/awesome-jev) | Potential discovery/distribution channel |

## Research boundaries

The subsequent [100-project survey](survey/README.md) adds a targeted static review of 100 distinct public repositories, with [per-project findings](survey/projects.md) and a [commit-pinned manifest](survey/manifest.json) covering 531 collected files. Collection is broader than the excerpt-level manual review. Related ports and benchmark/report projects are identified; these are not 100 independent customers or verified production deployments. The survey's proposed benefits remain hypotheses.

Searches covered Jev proxying, dashboards, observability, pricing, SDK configuration, and ecosystem projects. Independent Jev guide sites helped discover sources but were not treated as official TypeSafe documentation. Search visibility is not market size or uniqueness evidence.

Not established: repeat user adoption, production retention, comparative usability, console feature coverage behind login, current support in every framework, precise telemetry storage requirements, achievable overhead, or name availability. Recheck changing integrations and prices immediately before implementation or public claims.

No external outreach or publishing occurred. No third-party code was executed and no paid API request was made.
