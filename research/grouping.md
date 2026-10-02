# Automatic grouping of recurring Jev questions

Status: proposed feature, not implemented. Scope is Jev's Choice, Score, and Noul questions. There is no generic structured-output detector or additional inference step.

Revised after the [100-project survey](survey/README.md). In that sample, 18 projects primarily used indexed/contextual question instances and 26 primarily used changing candidate sets. Automatic grouping needs both recurring definition series and inspectable question families; exact JSON matching alone is insufficient.

## User experience

Point an existing Jev client at the local proxy. As requests arrive, the dashboard recognizes repeated question definitions and creates a group with suitable charts. Different input states and selected answers accumulate as observations in that group.

For example, repeated `department` Choice questions offering `billing`, `technical`, and `sales` would produce a department-routing group. Users could see choice percentages, uncertainty, request latency, and associated spend without configuring charts or tagging each call.

The request already describes the question; the proxy uses it to interpret the answer. Response shape alone is insufficient because Jev answers of the same primitive share their structure across unrelated tasks.

## Recognition rules

1. Assign incoming traffic to a source namespace. A single application can use the default namespace with no additional setup. Multiple applications can optionally use separately configured local routes or credentials. If callers share an indistinguishable source, the tool cannot infer application identity.
2. Match the answer to its question key within the intercepted request.
3. Build a deterministic definition fingerprint from the complete question object, including type, instructions, criteria, and any additional fields. Canonicalize ordinary JSON object-key order and insignificant serialization whitespace. Preserve string content, value types and array order. Also record a presentation fingerprint preserving object-member order, including criteria order: ordering can be an experimental variable. Keep the original representation when retention allows it.
4. Use source namespace, question key, definition fingerprint and presentation version as the automatic statistical series identity. Exclude input examples, results, timestamps and neighboring questions. Adding another question to a batch should not reset the existing group's history. A canonical definition can collect several presentation versions, but show their series separately by default.
5. Retain requested and returned model identifiers as filter dimensions. Show versions separately in comparisons and label any combined view. A model change does not erase a recurring question's history.

Use the question key as the initial display name. Users can rename a group locally without changing requests. No model-generated category names or background Jev calls are required.

An exact definition is an observable identity, not proof that the task's meaning stayed fixed. Some projects put rules, candidate descriptions or taxonomies in `state` and refer to them from the question. Support optional proxy-local `task_version` metadata or a configured extraction of specific state paths that carry rule configuration. Include that discriminator in the series identity. Do not fingerprint the entire state, because changing input examples should still accumulate together. Where this metadata is unavailable, identify the chart as a definition-based group with unverified task-version context. Arbitrary state semantics cannot be inferred reliably without application knowledge.

## Similar definitions and changes

Recognize recurring exact definitions automatically. If the same source and question key return with changed instructions or criteria, create a separate definition version and show a “definition changed” indicator. Present versions together for discovery, with distinct statistical series by default.

Matching primitive types and option names can suggest related groups, but cannot establish equal meaning. Let users link those groups under a local display folder while preserving their original definitions and counts. Renamed keys can be linked the same way. Never silently pool definitions just because their output JSON looks alike.

For dynamic candidate lists, changed candidates produce distinct definitions and a candidate-set fingerprint. Offer a family view for request volume, latency, candidate count and uncertainty, but keep option-frequency statistics scoped to the applicable candidate set. IDs such as `a0` or `c1` must not become global semantic labels. Cross-set action categories require an explicit mapping; preserve its version and show when candidate descriptions were not retained.

## Indexed question families

The survey gives concrete fixtures: jgrep row/chunk questions, compaction questions containing tool-call IDs and lengths, and per-pixel paint questions. A family groups their reusable operation while preserving every original definition and instance reference.

1. Start with source-specific, versioned local adapters for one or two reviewed request builders. An adapter recognizes an explicit request structure and separates an instance reference from the retained rule, rubric and task version.
2. Preserve the raw question key and definition fingerprint. Store the adapter ID/version, family ID, instance reference and mapping reason alongside them. Never rewrite the upstream request.
3. Aggregate only the metrics justified by the mapping. Equivalent row predicates can share a Noul histogram; dynamic candidate IDs cannot share option frequencies without stable semantics.
4. For unknown builders, suggest relationships based on a visible structural diff. Users can link them locally, with separate series until an explicit compatible mapping exists. A changed threshold, age, monetary limit or rule number must never disappear through blanket number stripping.
5. If a request contains many instances, count each valid answer once in answer distributions and deduplicate parent requests for associated latency/cost. Show both denominators. Offer per-request summaries as a separate view so large batches do not silently dominate a request-level statistic.

Bound discovery and suggestion work. Avoid creating thousands of visible charts for unrecognized pixel or row IDs. Keep an unclassified stream and surface bounded family suggestions, with retention and cardinality counters. Raw observations remain distinct until a supported mapping is available.

## Automatic charts

| Jev question type | Default statistics |
|---|---|
| Choice | Answer count, selected-option frequencies, average reported option probabilities, confidence histogram, trends over time |
| Score | Score histogram, mean and percentiles within the same rubric, level probabilities, confidence distribution |
| Noul | Yes-probability histogram and trend; above/below-threshold counts only with a visible configured threshold |
| Every group | Last seen, answer count, distinct request count, associated request latency, token usage, request-level cost basis, model/version breakdown |

Show denominators, time window, definition version, and available sample count. Keep missing/invalid answers separate from valid observations. A provider failure is not a negative Noul answer. An observed probability or confidence value is not a measured accuracy rate; correctness needs labels.

Latency, tokens, and cost are measured for the parent request. Label them as associated request metrics. Deduplicate parent request IDs within each group; if one request appears in several groups, those groups' costs overlap and must not be summed as an account total. Global totals come from the request ledger. Do not invent exact per-question billing.

## Local storage and controls

Store group metadata and aggregate statistics locally. Computing definition fingerprints does not require retaining raw input state. Apply the existing redaction and retention settings to question text and criteria as well. Fingerprints remain sensitive metadata, not a claim of anonymization.

Allow renaming, hiding, filtering, and linking related groups without changing captured events. Keep grouping-algorithm versions so an upgrade can explain changed group identities. Users can delete local data and export their own statistics. Grouping adds no subscription, external analytics, or model-call cost.

Proxy traffic and imported local logs use the same event model. Preserve import provenance and explicit source event IDs; deduplicate only when identity is established, not because two payloads match. Application actions, cache reuse, heuristic fallbacks, user corrections and labeled outcomes are separate event kinds. Missing application data stays unknown. The collector must never invent those events from a Jev answer alone.

## Verification examples for implementation

- Same definition, different tickets and answers: one group with accumulating observations.
- Changed ordinary JSON key ordering: same canonical definition but a recorded presentation version; reordered criteria or Score levels remain distinguishable in charts.
- Same option names with different descriptions or instructions: separate definitions.
- Same `noul` shape for unrelated questions: separate groups.
- Add an unrelated question to a batch: existing question identity stays stable.
- One request with three answers: three answer observations, one global request/cost record.
- Missing answer or failed request: visible failure/unknown state, excluded from valid-answer distributions.
- Same input replayed: another observation, not silently deduplicated as the original call.
- Same definition referring to a changed state-carried rule: separate series when a task-version discriminator is supplied; otherwise task equivalence remains unverified.
- Twenty indexed rows under one recognized rule: one family, twenty answer observations and one parent request record.
- Unknown number changes in instructions: separate definitions and an inspectable suggestion, never silent normalization.
- Same browser option ID referring to different elements: separate candidate context, no pooled label frequency.
- Reimport the same explicit source event ID: no duplicate; two independent calls with identical content: two observations.
