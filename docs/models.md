# Decision models and connection paths

Observer's **Connect an application → Decision models** catalog combines the [October 3 research snapshot](../research/models/README.md) with the [October 23 additions](../research/models/2026-10-23.md). Each entry has its own review date. It includes hosted services, open decision models, library imports and local runtimes. Choosing an entry shows setup instructions; it does not change the running upstream. Each Observer process still has one fixed upstream. Use separate processes, ports and databases to collect from several servers concurrently, or import their captures into one workspace.

A catalog entry establishes a source-reviewed integration path. It does not establish model quality or a successful live inference test. Existing live evidence covers only Jev through OpenRouter and Laya. Synthetic fixtures test Observer's interpretation without loading weights or validating model servers; the [research evidence](../research/models/2026-10-23-sources.json) distinguishes documented facts from runtime assumptions. Model names are suggestions from reviewed sources, not an account-specific availability list. Any other model name can still be entered.

## Compatible HTTP servers

Start your model server using its own instructions. With your saved `JEV_OBSERVER_DB_KEY` set, start Observer with the endpoint and provider ID shown in the catalog. For example:

```sh
# An already-running Kev server on loopback.
jev-observer --upstream http://127.0.0.1:8009/v1/systemone \
  --provider kev --upstream-auth none

# A closed hosted decision model with the compatible endpoint.
jev-observer --upstream https://api.milliseconds.ai/v1/systemone
```

For the local example, authenticate the SDK to Observer using the workspace token. For a hosted or key-protected server, register its bearer key in the dashboard and use the local client token. An authenticated local server needs `--upstream-auth bearer`. Point your SDK at Observer's origin, choose the model accepted by your upstream and keep `Accept-Encoding: identity`. See [connection and credential handling](connection.md).

Connection settings have three separate identities. `--provider` identifies the credential/billing provider. `--model-profile` selects the reviewed model/runtime validation contract. `--protocol` selects `systemone`, `openai-decisions` or `cloudflare-systemone`; the latter two preserve their native request/response shape while adapting it for inspection. The catalog supplies recommendations independently. Without an explicit profile, provider attribution retains the legacy profile fallback. Unknown explicit profile IDs are rejected. An unknown provider ID without an explicit profile uses strict System One validation; it does not enable arbitrary shapes. Observer preserves reported values rather than renormalizing distributions, synthesizing confidence or inventing usage and prices.

For example, local Clef served by Ollama uses `--provider ollama --model-profile ollama-clef --protocol systemone`. Its reviewed local runtime allows 26 choices and preserves full floating-point values; the native Clef Python helper rounds to four decimals. A Workers AI connection uses `--provider cloudflare --model-profile cloudflare-clef --protocol cloudflare-systemone` and the account-scoped REST URL with your actual account ID. Its `result`/`success` envelope is separate from the direct Worker binding return value.

OpenAI Decisions uses `--provider openai --model-profile openai-decisions --protocol openai-decisions` with `https://api.openai.com/v1/decisions`. Its questions and answers are ordered arrays, Choice values can be typed strings or booleans, and definitions use native `choices` and `levels`. Do not preconvert these fields to System One. The current HTTP reference documents 1–400 questions, while the pinned Python SDK docstring says 1–200; the catalog does not invent a missing Score-level limit. See [the official API reference](https://developers.openai.com/api/reference/resources/decisions/methods/create).

Differences handled by profiles include Kev/Decider/Laya four-decimal distributions, milliseconds.ai three-decimal distributions, Kev's wider rubrics, Nimble's 26-level rubrics, single-level scores in some runtimes, structured legends and JevK5's absent legend. Eikos reports a modal Score and a separate `expected` value; Observer keeps the reported Score and marks its meaning in a warning. Confidence formulas vary by model. Filter to one model when interpreting confidence summaries; these fields are not measured accuracy or directly comparable calibration scores.

System One supports named Choice, Noul and ordinal Score decisions, plus Box for explicitly reviewed profiles. Chat completions, extraction, ranking, session APIs and custom batch endpoints are separate interfaces. Laya's numeric/boolean Choice labels remain outside the System One string-label view. The catalog supports Laya's regular named decisions. Recognized input envelopes, including `assets`, `media` and `videos`, follow the input-capture preference; the proxy still forwards the original bytes. Assets used inside question instructions or criteria affect question meaning and must not silently join unrelated definitions.

TypeSafe Jev 1.14's experimental Box reports normalized `top`, `bottom`, `left` and `right` edges with confidence. H2O Lightning additionally reports `found` and sometimes `present`; its no-localization result can be a full-frame box with zero confidence and `found:false`. A full-frame shape by itself is not evidence of absence. Box edges are not a scalar score and should not be averaged as one.

OneJev 1.6, older OmniJev v1.1, and akhilaaa3's Jev-Omni are different projects. OneJev's qev server is a System One path with six-decimal results, optional instructions and nullable rubric descriptions. The older OmniJev library has abstention and modal Score semantics; Jev-Omni is a single-question classifier. Their offline adapters are described below. Liquid d1 hosted, Liquid open libraries, Perplexity v1.1/v1.2, Strands and H2O also have distinct runtime profiles; selecting a similarly named model in a different runtime does not establish compatibility.

## Library results and custom APIs

Use **Import records → Decision request/response capture** for decisions collected outside Observer. This covers SemIf, Tev1, NanoJev, AgentJev, AnyJev, AutoTrust JEV, Jev-Omni, NeoHorse, Intern-Decision, FRIDA, vLLM Semantic Router models, Jev-Style GGUF, Verdict, Valen, dev-0.4b and Blink. These integrations require an explicit mapping of the original questions and reported results. The catalog calls them *mapped capture imports*. It does not claim their native API is directly proxied.

The compatibility format ID remains `systemone-capture`. System One is its default protocol; native OpenAI Decisions and wrapped Cloudflare captures can supply `protocol` and `model_profile` while preserving their original request and response.

One JSON object per line represents one original call:

```json
{"id":"app-call-0001","timestamp":1790985600000,"source":"my-app","provider":"nanojev","status":200,"duration_ms":null,"capture_complete":true,"request":{"questions":{"urgent":{"type":"noul","instructions":"Is this urgent?"}}},"response":{"answers":{"urgent":{"type":"noul","noul":0.7}}}}
```

`id` is a stable event identity from your application, and `timestamp` is its original Unix time in milliseconds. `source` and `provider` namespace the identity. Reimporting the same event is a duplicate; an independent call needs a new ID even if its payload matches. `status` records the HTTP status, or 200 for a successfully completed local library call. Set `capture_complete` false for an incomplete observation. `duration_ms` may be null, and a missing reported model or token count remains unknown. Optional `task_version` identifies rule versions carried in state. Set `sample: true` for synthetic examples. Derived validity, costs, group IDs and caller-provided labels are not trusted.

Imports accept at most 8 MiB of decoded text and 10,000 records. Normalization has separate budgets of 16 MiB per record and 64 MiB per batch, applied to both generated JSON and work such as copying task versions into answers or filling annotation provenance. Excessive expansion returns 413 and saves none of the batch. Metadata and identities are never shortened to fit; reduce a single oversized observation at its source, or split a large batch. Some historical captures or exports may exceed these import limits. These limits do not change live forwarding or its capture settings.

Observer JSONL records allow up to 144 nested objects or arrays, counting the record itself. This bounded allowance includes Observer's added wrappers around retained raw fields. Excessive nesting returns 400 and saves none of the batch. Raw provider bodies and System One capture/receipt imports keep their existing parser depth limit.

For System One captures, `request.questions` must retain the named definitions, including the complete candidate domain and ordered rubric. Native OpenAI Decisions captures use the supported question and answer arrays described above. `response.answers` uses `type` plus `choice`, `noul` or `score`. Choice and Score need the complete reported probability map; a winning label or truncated top-k logprobs do not establish that distribution. Known mapped-model profiles allow omitted confidence and legends with visible warnings. Malformed fields that are present still fail validation. Unknown provider IDs need the full strict response contract.

The offline converter handles System One envelopes and the reviewed NanoJev, AgentJev, AnyJev, SemIf, AutoTrust, OmniJev and Jev-Omni result shapes:

```sh
python3 scripts/capture-decision.py \
  --request named-systemone-definitions.json --response saved-results.json \
  --dialect nanojev --provider nanojev --source my-app \
  --id app-call-0001 --timestamp 1790985600000 \
  --state-id original-state-id --output capture.jsonl
```

The request file must be the corresponding named definitions expressed as System One questions. For NanoJev/AgentJev, map the original `boolean` question to Noul and retain its original wording and criteria; do not substitute its thresholded boolean for the probability. Batch files require a state ID unless they contain exactly one result. The converter joins by explicit question IDs, keeps original results, refuses duplicate/mismatched IDs and never calls a model. It refuses overwriting an existing output file. Raw converter files can contain input and model content; imported history follows Observer's capture and redaction settings. The converter keeps response input fields (`state`, `input`, `images`, `image`, `audio`, `video`, `videos`, `media`, `assets`) in the response envelope so input capture controls them. Imports also apply that preference to recognized fields in older converters' `original_metadata`, including when reimporting Observer exports.

Use `--dialect omnijev` for the older MSO library's direct question-keyed result mapping. Choice `abstain` and `valid` remain reported fields; any abstention mass or `valid:false` makes the observation invalid, even when the missing mass fits rounding tolerance. Nothing is renormalized. For Score, supply the exact original native level IDs as distinct strings in the System One `criteria` array, in their original rubric order. The converter requires an exact match to the reported probability keys, then maps the reported modal level to its ordinal index. It never computes an expected Score. An invalid native empty distribution remains inspectable; an unknown or ambiguous level mapping is rejected.

Use `--dialect jev-omni` for one original classifier call. Supply one Choice question whose criteria keys retain the exact original option order. The reported `prediction`, zero-based `prediction_index` and probability-map order must agree with it. The converter accepts 2–255 options; the native 256-option case is rejected explicitly at Observer's inspection boundary. It does not infer a Noul or Score from a classifier label. Both Omni dialects set their own `model_profile` while preserving your `--provider`; `--model-profile` can explicitly select another reviewed contract for saved System One results.

Other mappings are explicit application work:

| Result format | Mapping to a capture |
|---|---|
| AnyJev | `probability` → Noul, `answer` plus `distribution` → Choice. Preserve calibration `level`. Numeric bin centers must be mapped explicitly to ordinal indices before recording a Score. |
| SemIf | `option_ids` plus the complete probability array → named Choice distribution; the converter marks the derived argmax and retains probability-status metadata. |
| AutoTrust | `options` plus the complete probability array → named distribution; retain the reported choice. Map non-Choice results to Noul/ordinal Score using the original question contract. |
| Tev1 / Jev-Style GGUF | Map answer letters to semantic option IDs using the original prompt. A selected letter alone, or a capped `top_logprobs`, cannot supply the full distribution. |
| NeoHorse / Intern-Decision / FRIDA / vLLM Semantic Router | Map runtime results by question ID, preserving the original candidate order. Preserve any checkpoint/revision and probability-status metadata as extensions. FRIDA ranking results are outside these three views. |
| Verdict / Valen / dev-0.4b / Blink | Map the library's option probabilities to the original named candidate domain. Supply the original question definition and use zero-based rubric indices for Score. |

Exports use Observer JSONL, which retains raw answers and recomputes validation on reimport. Costs remain unknown unless an explicitly configured estimate can be computed from actual token usage, or the verified OpenRouter USD cost field is present. A custom runtime's `usage` counters are not automatically billed tokens.
