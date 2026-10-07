# Current Jev-like models: research and coverage

Checked **October 3, 2026**. This survey identifies models that make bounded decisions over supplied candidates, conditions or ordered rubrics. The dashboard catalog contains **38 families and serving integrations**: 22 compatible HTTP paths and 16 paths through mapped capture imports. A family includes its size variants, revisions and aliases; provider gateways and runtimes are listed separately because their connection and credential requirements differ.

## Research method and limits

Discovery used current [awesome-jev](https://github.com/cobanov/awesome-jev), [awesome-decision-models](https://github.com/sfmqrb/awesome-decision-models), official provider docs and Hugging Face's live `trendingScore` ordering for the searches `jev`, `decision`, `laya` and `kev` (top 15 each). We then inspected project-authored READMEs, model cards and selected server/schema/readout code. [sources.json](sources.json) records the ordered discovery results, 27 repository commits and 148 fetched file hashes, plus 11 pinned Hugging Face cards. Full third-party source remains outside this repository. Fetched-file inventory is broader than the excerpts manually reviewed; this is not an audit of every source line.

“Trending” here means discoverable in those current trend lists or prominent/recently active in this decision-model ecosystem. Stars, likes and downloads are snapshot signals, not measurements of user growth, independent deployments or accuracy. Laya (30,286 stars), Kev (8,339), SemIf (4,663), NanoJev (2,478), Nimble (2,032), Ollaya (1,138), Decider (1,047) and AnyJev (1,018) had substantial visible GitHub interest at collection time. The Hugging Face sample also surfaced Jev-Omni, AutoTrust JEV including vision, NeoHorse, Intern-Decision, MATILDA-jev and newer vLLM Semantic Router decision checkpoints.

We retained primary sources for implementation claims. Discovery articles and directories were used to find candidates, not to infer API compatibility or benchmark superiority. False positives such as DecisionIntelligence/Aurora (time-series forecasting) and unrelated `kev` substring matches were excluded. Quantizations, hardware ports and size variants are covered under their parent family where they retain its decision contract. Current lists contain more experiments than this reviewed set; this snapshot is not a claim to enumerate every public decision checkpoint.

No paid model calls, model downloads, external outreach or third-party training/inference code execution occurred. “Open” denotes an open model, library or runtime; each artifact's code, base weights, checkpoint and data terms must be read separately. The manifest's GitHub license field is metadata for the repository, not a conclusion about every weight file.

## Findings that changed implementation

1. Closed alternatives are real interfaces to cover. [milliseconds.ai](https://docs.milliseconds.ai/typesafe/overview) exposes decision-machine-1 through the same endpoint with its own model name, limits and confidence semantics. TypeSafe currently documents Jev 1.13.0 and two aliases; there is no separate currently documented Jev-Ultrafast model in its model list. OpenRouter and Vercel are access paths to Jev, and Codiv is a hosted access path to open models.
2. Endpoint names alone do not establish response compatibility. Kev, Decider and Laya independently round distributions; milliseconds.ai displays three-decimal distributions. Strict Jev tolerances could exclude valid observations. The new catalog profiles use explicit bounds derived from each reviewed serialization precision and retain original values.
3. Rubrics and legends differ. Kev allows up to 255 Score levels; the reviewed Nimble wrapper allows 26. Laya supports one to 32, some other servers allow one level, and several retain JSON legends. JevK5 omits Score legends. Eikos reports a modal Score plus a separate expected value. These differences need validation and visible interpretation, rather than silently rewritten results.
4. Several prominent projects do not expose the same HTTP contract. SemIf and AnyJev have library/CLI results; NanoJev and AgentJev have custom batched `/api/evaluate`; AutoTrust exposes `/v1/decide`; FRIDA uses a vLLM pooling surface. The new raw-capture format makes their mapped decisions usable in the same request, grouping, review and export views. It requires explicit definitions and full probability correspondence. No universal provider routing or automatic conversion of arbitrary library code is claimed.
5. Vision inputs can hide in request extensions. The capture-state preference now also excludes recognized envelope media fields by default. Raw forwarded bytes remain untouched.
6. Confidence and token counters have model-specific meanings. Missing confidence stays unknown, while native reported confidence is preserved. Custom response counters remain extensions unless they are documented native input/output tokens. No price is copied from search results or treated as an invoice.

## Reviewed coverage

| Primary source | Provider ID | Access | Dashboard path | Snapshot signal |
|---|---|---|---|---|
| [Jev](https://docs.typesafe.ai/models) | typesafe | closed | System One proxy | Official documentation / model card |
| [decision-machine-1](https://docs.milliseconds.ai/typesafe/overview) | milliseconds | closed | System One proxy | Official documentation / model card |
| [Jev through OpenRouter](https://openrouter.ai/typesafe/jev-1.13) | openrouter | closed | System One proxy | Official documentation / model card |
| [Jev through Vercel AI Gateway](https://vercel.com/changelog/ai-gateway-now-supports-typesafe-clients-and-http-api-for-jev) | vercel | closed | System One proxy | Official documentation / model card |
| [Laya](https://github.com/NandhaKishorM/laya/blob/fa9a2a7070b1789912a49ae24603bbfb1a78b001/README.md) | laya | open | System One proxy | 30,286 GitHub stars |
| [Kev](https://github.com/jaredpalmer/kev/blob/84847f0a883d900f7de5b7a57eaa341ca7f9a6b4/README.md) | kev | open | System One proxy | 8,339 GitHub stars |
| [Decider](https://github.com/Mapika/decider/blob/45024082b7d4bb667bf9140b7a7c073d3b483f6b/README.md) | decider | open | System One proxy | 1,047 GitHub stars |
| [Bespoke Nimble](https://github.com/bespokelabsai/nimble/blob/62076b4f2d365b5879dafcf7f6dd072a1fe76df7/README.md) | nimble | open | System One proxy | 2,032 GitHub stars |
| [Von](https://github.com/wfzyx/von/blob/ef4e207beabe7abd695e4dbe01c6d2d3b637b9c4/README.md) | von | open | System One proxy | 821 GitHub stars |
| [OpenJev (DiffusionGemma)](https://github.com/razorback16/openjev/blob/dcd20947b5ddad5be4a8f5aed6aa6dd245653823/README.md) | openjev | open | System One proxy | 591 GitHub stars |
| [Open models through Codiv](https://github.com/razorback16/openjev/blob/dcd20947b5ddad5be4a8f5aed6aa6dd245653823/README.md) | codiv | open | System One proxy | 591 GitHub stars |
| [Open-Jev (Zefan Cai)](https://github.com/Zefan-Cai/Open-Jev/blob/7434a4a8572a603067e3355799a72e1051b83b8a/README.md) | open-jev | open | System One proxy | 389 GitHub stars |
| [Reflex](https://github.com/kshetrajna12/reflex/blob/231f896d818a62b94fec305ed553df1088486dcb/README.md) | reflex | open | System One proxy | 161 GitHub stars |
| [JevK5](https://github.com/allebee/jevk5/blob/f26426d16f59e8bbe1470e5b162cc89329e29b29/README.md) | jevk5 | open | System One proxy | 131 GitHub stars |
| [OpenThai-SystemOne](https://github.com/iapp-technology/openthai-systemone/blob/5d04bcca0c58bd10e7dac2d3d369d8f760bea6cf/README.md) | openthai | open | System One proxy | 65 GitHub stars |
| [Jeff (GliFormer)](https://github.com/logan-markewich/jeff/blob/34b32f99a727c47b679adde33f4702a001e02979/README.md) | jeff | open | System One proxy | 284 GitHub stars |
| [Rizzo Flow](https://github.com/Rizzo-AI-Academy/rizzo-flow/blob/b9ba007ee4d2928bbab5b1d8bfe9009c3696b6de/README.md) | rizzo | open | System One proxy | 798 GitHub stars |
| [Ollaya](https://github.com/ollaya-dev/ollaya/blob/37fcfa9f8a35b6b389447ffca49e4b4242970f81/README.md) | ollaya | open | System One proxy | 1,138 GitHub stars |
| [EdgeJev](https://github.com/yzfly/edgejev/blob/dce469472e0e6eea396256e9bf866082dcb9490b/README.md) | edgejev | open | System One proxy | 15 GitHub stars |
| [OpenJev on SGLang](https://github.com/ekzhang/openjev-sglang/blob/bf6a53bbb1f75040b06de39abff1230524553ab5/README.md) | openjev-sglang | open | System One proxy | 337 GitHub stars |
| [SemIf (formerly OpenJev)](https://github.com/TheoLeeCJ/SemIf-OpenJev/blob/23cf1f39fc9534fe81437200959b6dfc7106e45a/README.md) | semif | open | Mapped capture import | 4,663 GitHub stars |
| [Tev1](https://github.com/togethercomputer/tev1/blob/1dde7782382c9f49d627153759b8d1deab426ce0/README.md) | tev1 | open | Mapped capture import | 205 GitHub stars |
| [NanoJev](https://github.com/TianyuCodings/NanoJev/blob/76fdfc9ecdca45a9bcef17991a07d3041a87685a/README.md) | nanojev | open | Mapped capture import | 2,478 GitHub stars |
| [AgentJev](https://github.com/malevrigns/agent-jev/blob/1c2c1b1ae1dc427d4cc851ef7460a1112b0cb3e1/README.md) | agentjev | open | Mapped capture import | 335 GitHub stars |
| [AnyJev](https://github.com/nokia-applied-research/AnyJev/blob/e6efe1233a57ddc8b291eeb206142bcec003d10b/README.md) | anyjev | open | Mapped capture import | 1,018 GitHub stars |
| [Verdict](https://github.com/Heman10x-NGU/openJev-verdict-2.0/blob/bff28567cff463b833bf044f351a8b7945d53e07/README.md) | verdict | open | Mapped capture import | 293 GitHub stars |
| [Valen](https://github.com/Liuziyu77/Valen/blob/33de9f9f77a00120009f1dd253cf06e32adc348f/README.md) | valen | open | Mapped capture import | 574 GitHub stars |
| [Eikos](https://github.com/caiovicentino/eikos/blob/8902fbe9e06e10da50d3ed5d27bb52fcf8ddd900/README.md) | eikos | open | System One proxy | 40 GitHub stars |
| [dev-0.4b](https://github.com/mpnikhil/dev-0.4b/blob/2ce2563db938e9ec22ffe69e5fb8d4706f61ed1a/README.md) | dev | open | Mapped capture import | 44 GitHub stars |
| [Blink](https://github.com/sqliteai/blink/blob/18d6ce836cadfd14f04feef8bba952f50da660db/README.md) | blink | open | Mapped capture import | 19 GitHub stars |
| [Jev-Omni](https://huggingface.co/akhilaaa3/Jev-Omni/blob/5addda86ddee081a68fb067477ea100c221b8917/README.md) | jev-omni | open | Mapped capture import | Official documentation / model card |
| [AutoTrust JEV](https://huggingface.co/autotrust/JEV-27B-VL/blob/d835ee0b42460bd74890c7eab63ba0dcb4dd808a/README.md) | autotrust | open | Mapped capture import | Official documentation / model card |
| [NeoHorse-Jev](https://huggingface.co/TokenRhythm/NeoHorse-Jev-4B/blob/434cb21d3a994a953d3ae5788405fcb2c4970554/README.md) | neohorse | open | Mapped capture import | Official documentation / model card |
| [MATILDA-jev](https://huggingface.co/Maincode/matilda-jev-v1/blob/c87f57504300588ff4270ff5ac17c38d1c7996b9/README.md) | matilda | open | System One proxy | Official documentation / model card |
| [Jev-Style GGUF](https://huggingface.co/chaoliangUNSW/Jev-Style-Qwen3.5-2B-Decision-GGUF/blob/adc5656741715ddd3e40dac44c6294cb888bc655/README.md) | jev-style | open | Mapped capture import | Official documentation / model card |
| [Intern-Decision](https://huggingface.co/internlm/Intern-Decision-4B/blob/0e5e6aa7d6d750e2b1504ba11a8136cb58aeb3cd/README.md) | intern-decision | open | Mapped capture import | Official documentation / model card |
| [FRIDA-Decisions](https://huggingface.co/ai-forever/FRIDA-Decisions/blob/0096b5384e821c68791cb2b3b7292c2b937dcec8/README.md) | frida | open | Mapped capture import | Official documentation / model card |
| [vLLM Semantic Router Decision models](https://huggingface.co/vllm-sr/Decision-2.0-Vega-27B/blob/477e90f537eb5bd62e90d5e5361c7b654e69cf45/README.md) | vllm-sr | open | Mapped capture import | Official documentation / model card |

## Evidence and reproduction

The completed check results and their scope are recorded in [validation.json](validation.json).

See [model connections and capture mappings](../../docs/models.md) for the final contract and [src/model_catalog.json](../../src/model_catalog.json) for the offline catalog used by both configuration and the dashboard. The source register pins exact commits/revisions and content hashes; URLs can be fetched again without executing upstream code. The ordered Hugging Face discovery results are retained because ranking changes over time.

Verification added source-derived boundary checks for rounded probabilities, extended rubrics, missing legends/confidence, structured legends, malformed fields, redaction isolation and explicit event identities. A catalog-wide synthetic roundtrip checks all provider IDs through the capture pipeline. Python converter checks cover source-specific result joins, probability-array correspondence and distinctions between probabilities, thresholded booleans, confidence and numeric-bin scores. Browser checks exercise catalog selection and mapped capture import through the packaged backend. These establish Observer behavior; live inference for newly cataloged models remains unverified. Prior live checks for OpenRouter Jev and Laya are linked in [compatibility.md](../../docs/compatibility.md).

```sh
python3 scripts/test-decision-capture.py
cargo test --locked model::compatibility_tests
npm run build --prefix ui
cargo build --locked
OBSERVER_BINARY="$PWD/target/debug/jev-observer" npm run test:integration --prefix ui
```
