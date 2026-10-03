# Decision models and connection paths

Observer's **Connect an application → Decision models** catalog covers the [October 3, 2026 research snapshot](../research/models/README.md). It includes closed Jev and decision-machine-1, hosted gateways, open decision models and local runtimes. Choosing an entry shows setup instructions; it does not change the running upstream. Each Observer process still has one fixed upstream. Use separate processes, ports and databases to collect from several servers concurrently, or import their captures into one workspace.

A catalog entry establishes a documented integration path. It does not establish model quality or a successful live inference test. Existing live evidence covers only Jev through OpenRouter and Laya. All catalog entries are covered by synthetic normalization/export/import contract checks; the checks do not load their weights or validate their model servers. Model names are suggestions from the reviewed sources, not an account-specific availability list. Any other model name can still be entered.

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

Provider attribution also selects a source-reviewed validation profile. It is inferred from the exact TypeSafe, OpenRouter, milliseconds.ai, Codiv and Vercel hostnames; local servers need `--provider`. For a gateway to multiple model families, select the profile that matches the response contract of your configured runtime. Unknown provider IDs use the existing strict System One validation. Observer preserves reported values rather than renormalizing distributions, synthesizing confidence or inventing usage and prices.

Differences handled by profiles include Kev/Decider/Laya four-decimal distributions, milliseconds.ai three-decimal distributions, Kev's wider rubrics, Nimble's 26-level rubrics, single-level scores in some runtimes, structured legends and JevK5's absent legend. Eikos reports a modal Score and a separate `expected` value; Observer keeps the reported Score and marks its meaning in a warning. Confidence formulas vary by model. Filter to one model when interpreting confidence summaries; these fields are not measured accuracy or directly comparable calibration scores.

System One compatibility is limited to named Choice, Noul and ordinal Score decisions. Chat completions, extraction, ranking, session APIs and custom batch endpoints are separate interfaces. Laya's numeric/boolean Choice labels remain outside the supported string-label view. The catalog supports Laya's regular named decisions. Image-capable servers can receive their normal input extensions; `state`, `images`, `image`, `audio` and `video` at the request/response envelope are excluded from saved captures unless input capture is enabled. The proxy still forwards the original body.

## Library results and custom APIs

Use **Import records → System One request/response capture** for decisions collected outside Observer. This covers SemIf, Tev1, NanoJev, AgentJev, AnyJev, AutoTrust JEV, Jev-Omni, NeoHorse, Intern-Decision, FRIDA, vLLM Semantic Router models, Jev-Style GGUF, Verdict, Valen, dev-0.4b and Blink. These integrations require an explicit mapping of the original questions and reported results. The catalog calls them *mapped capture imports*. It does not claim their native API is directly proxied.

One JSON object per line represents one original call:

```json
{"id":"app-call-0001","timestamp":1790985600000,"source":"my-app","provider":"nanojev","status":200,"duration_ms":null,"capture_complete":true,"request":{"questions":{"urgent":{"type":"noul","instructions":"Is this urgent?"}}},"response":{"answers":{"urgent":{"type":"noul","noul":0.7}}}}
```

`id` is a stable event identity from your application, and `timestamp` is its original Unix time in milliseconds. `source` and `provider` namespace the identity. Reimporting the same event is a duplicate; an independent call needs a new ID even if its payload matches. `status` records the HTTP status, or 200 for a successfully completed local library call. Set `capture_complete` false for an incomplete observation. `duration_ms` may be null, and a missing reported model or token count remains unknown. Optional `task_version` identifies rule versions carried in state. Set `sample: true` for synthetic examples. Derived validity, costs, group IDs and caller-provided labels are not trusted.

`request.questions` must retain the named System One definitions, including the complete candidate domain and ordered rubric. `response.answers` uses `type` plus `choice`, `noul` or `score`. Choice and Score need the complete reported probability map; a winning label or truncated top-k logprobs do not establish that distribution. Known mapped-model profiles allow omitted confidence and legends with visible warnings. Malformed fields that are present still fail validation. Unknown provider IDs need the full strict response contract.

The offline converter handles System One envelopes and the reviewed NanoJev, AgentJev, AnyJev, SemIf and AutoTrust result shapes:

```sh
python3 scripts/capture-decision.py \
  --request named-systemone-definitions.json --response saved-results.json \
  --dialect nanojev --provider nanojev --source my-app \
  --id app-call-0001 --timestamp 1790985600000 \
  --state-id original-state-id --output capture.jsonl
```

The request file must be the corresponding named definitions expressed as System One questions. For NanoJev/AgentJev, map the original `boolean` question to Noul and retain its original wording and criteria; do not substitute its thresholded boolean for the probability. Batch files require a state ID unless they contain exactly one result. The converter joins by explicit question IDs, keeps original results, refuses duplicate/mismatched IDs and never calls a model. It refuses overwriting an existing output file. Raw converter files can contain input and model content; imported history follows Observer's capture and redaction settings.

Other mappings are explicit application work:

| Result format | Mapping to a capture |
|---|---|
| AnyJev | `probability` → Noul, `answer` plus `distribution` → Choice. Preserve calibration `level`. Numeric bin centers must be mapped explicitly to ordinal indices before recording a Score. |
| SemIf | `option_ids` plus the complete probability array → named Choice distribution; the converter marks the derived argmax and retains probability-status metadata. |
| AutoTrust | `options` plus the complete probability array → named distribution; retain the reported choice. Map non-Choice results to Noul/ordinal Score using the original question contract. |
| Tev1 / Jev-Style GGUF | Map answer letters to semantic option IDs using the original prompt. A selected letter alone, or a capped `top_logprobs`, cannot supply the full distribution. |
| Jev-Omni / NeoHorse / Intern-Decision / FRIDA / vLLM Semantic Router | Map runtime results by question ID, preserving the original candidate order. Preserve any checkpoint/revision and probability-status metadata as extensions. FRIDA ranking results are outside these three views. |
| Verdict / Valen / dev-0.4b / Blink | Map the library's option probabilities to the original named candidate domain. Supply the original question definition and use zero-based rubric indices for Score. |

Exports use Observer JSONL, which retains raw answers and recomputes validation on reimport. Costs remain unknown unless an explicitly configured estimate can be computed from actual token usage, or the verified OpenRouter USD cost field is present. A custom runtime's `usage` counters are not automatically billed tokens.
