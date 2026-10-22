//! Local normalization. No inference, network requests, or model-derived labels.
//!
//! Native types: https://docs.typesafe.ai/api (reviewed 2026-09-22).
//! jgrep-v1 recognizes only the literal prefix in the reviewed buildRequest:
//! https://github.com/kyu1204/jgrep/blob/e18f36bdf4fb24e60ac43c7435ef54b0997f383c/src/jgrep.ts
//! Receipt adapter: JevRouter f944acb6530621bced023352e2358a63218bf4d9,
//! src/types.ts RouteResult and src/store.ts saveDecision. Receipts do not
//! contain the original complete question definitions or necessarily timings.

use crate::catalog::{self, Profile};
use aho_corasick::{AhoCorasick, AhoCorasickKind, MatchKind};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const REDACTED: &str = "[REDACTED]";
const INPUT_FIELDS: [&str; 5] = ["state", "images", "image", "audio", "video"];
const MAX_IMPORT_RECORDS: usize = 10_000;
const PROBABILITY_TOLERANCE: f64 = 0.0001;

#[derive(Clone, Copy)]
struct RedactionLimits {
    patterns: usize,
    pattern_bytes: usize,
    work: usize,
}

const REDACTION_LIMITS: RedactionLimits = RedactionLimits {
    patterns: 32_768,
    pattern_bytes: 1024 * 1024,
    work: 64 * 1024 * 1024,
};

const REDACTION_LIMIT_MESSAGE: &str =
    "Credential redaction exceeded its work limit; request and response content were not retained";

/// Match the dashboard's JavaScript Date domain and HTTP date filters.
pub fn valid_timestamp(timestamp: i64) -> bool {
    (-8_640_000_000_000_000..=8_640_000_000_000_000).contains(&timestamp)
}

#[derive(Debug)]
pub struct ImportLimitError(&'static str);

impl std::fmt::Display for ImportLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Import exceeds the {} limit", self.0)
    }
}

impl std::error::Error for ImportLimitError {}

#[derive(Clone, Copy)]
struct ImportLimits {
    record: usize,
    batch: usize,
}

const IMPORT_LIMITS: ImportLimits = ImportLimits {
    record: 16 * 1024 * 1024,
    batch: 64 * 1024 * 1024,
};

fn limited_add(
    current: usize,
    additional: usize,
    limit: usize,
    kind: &'static str,
) -> Result<usize> {
    current
        .checked_add(additional)
        .filter(|total| *total <= limit)
        .ok_or_else(|| ImportLimitError(kind).into())
}

/// Count serialized bytes without allocating a second copy of the JSON.
fn json_size<T: serde::Serialize + ?Sized>(
    value: &T,
    limit: usize,
    kind: &'static str,
) -> Result<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|total| *total <= self.limit)
                .ok_or_else(|| std::io::Error::other("JSON budget exceeded"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| ImportLimitError(kind))?;
    Ok(counter.bytes)
}

struct ImportBudget {
    limits: ImportLimits,
    work: usize,
    normalized: usize,
    redaction_remaining: usize,
}

struct ImportRecordBudget<'a> {
    batch: &'a mut ImportBudget,
    work: usize,
}

impl ImportRecordBudget<'_> {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        let record = limited_add(
            self.work,
            bytes,
            self.batch.limits.record,
            "normalization work",
        )?;
        let batch = limited_add(
            self.batch.work,
            bytes,
            self.batch.limits.batch,
            "normalization work",
        )?;
        self.work = record;
        self.batch.work = batch;
        Ok(())
    }

    fn repeated(&mut self, bytes: usize, count: usize) -> Result<()> {
        let bytes = bytes
            .checked_mul(count)
            .ok_or(ImportLimitError("normalization work"))?;
        self.charge(bytes)
    }

    fn size<T: serde::Serialize + ?Sized>(&self, value: &T) -> Result<usize> {
        json_size(value, self.batch.limits.record, "normalization work")
    }

    fn value(&mut self, value: &Value) -> Result<()> {
        self.charge(self.size(value)?)
    }

    fn structure(&mut self, record: &Value, format: &str) -> Result<()> {
        self.charge(512)?;
        if format == "systemone-capture" {
            for _ in answer_keys(&record["request"], &record["response"]) {
                self.charge(512)?;
            }
        } else if format == "observer-jsonl" {
            self.repeated(512, record["answers"].as_array().map_or(0, Vec::len))?;
            for key in ["actions", "labels"] {
                self.repeated(128, record[key].as_array().map_or(0, Vec::len))?;
            }
        }
        // Receipt answers are retained as provenance but never normalized into
        // inference answers, so their bytes are covered by the retained copies.
        Ok(())
    }

    fn finish(&mut self, record: &Value) -> Result<()> {
        crate::normalized_json::check_value(record)?;
        let bytes = json_size(record, self.batch.limits.record, "normalized output")?;
        self.batch.normalized = limited_add(
            self.batch.normalized,
            bytes,
            self.batch.limits.batch,
            "normalized output",
        )?;
        Ok(())
    }
}

fn answer_keys<'a>(request: &'a Value, response: &'a Value) -> impl Iterator<Item = &'a String> {
    let questions = request["questions"].as_object();
    questions.into_iter().flat_map(|map| map.keys()).chain(
        response["answers"]
            .as_object()
            .into_iter()
            .flat_map(|map| map.keys())
            .filter(move |key| questions.is_none_or(|map| !map.contains_key(*key))),
    )
}

#[derive(Clone, Debug, Default)]
pub struct Capture {
    pub id: String,
    pub timestamp: i64,
    pub source: String,
    pub task_version: Option<String>,
    pub adapter: Option<String>,
    pub status: u16,
    pub duration_ms: f64,
    pub request: Vec<u8>,
    pub response: Vec<u8>,
    pub capture_complete: bool,
    pub transport_error: Option<String>,
    pub secret: Option<String>,
    pub local_token: Option<String>,
    pub access_token: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct NormalizeOptions {
    pub provider: Option<String>,
    pub capture_state: bool,
    pub redact_keys: Vec<String>,
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
}

fn key_name(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn sensitive_key(key: &str, options: &NormalizeOptions) -> bool {
    let key = key_name(key);
    matches!(
        key.as_str(),
        "authorization"
            | "proxyauthorization"
            | "xobserveraccess"
            | "apikey"
            | "xapikey"
            | "typesafeapikey"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "clienttoken"
            | "sessiontoken"
            | "authtoken"
            | "bearertoken"
            | "idtoken"
            | "password"
            | "secret"
            | "clientsecret"
            | "secretkey"
            | "privatekey"
            | "credential"
            | "credentials"
            | "cookie"
            | "setcookie"
    ) || options.redact_keys.iter().any(|k| key_name(k) == key)
}

fn collect_secret_string(value: &str, found: &mut Vec<String>) {
    if value.is_empty() || value == REDACTED {
        return;
    }
    found.push(value.to_owned());
    if let Some((scheme, credential)) = value.split_once(char::is_whitespace)
        && scheme.eq_ignore_ascii_case("bearer")
    {
        let credential = credential.trim();
        if !credential.is_empty() {
            found.push(credential.to_owned());
        }
    }
}

fn collect_sensitive_values(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::String(value) => collect_secret_string(value, found),
        Value::Array(values) => {
            for value in values {
                collect_sensitive_values(value, found);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_sensitive_values(value, found);
            }
        }
        _ => {}
    }
}

fn collect_secrets(value: &Value, options: &NormalizeOptions, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if sensitive_key(key, options) {
                    // Header values and configured private fields can be
                    // arrays or objects. Their string contents must also be
                    // removed from echoes outside the sensitive container.
                    collect_sensitive_values(value, found);
                } else {
                    collect_secrets(value, options, found);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_secrets(value, options, found);
            }
        }
        _ => {}
    }
}

struct SecretMatcher<'a> {
    secrets: Vec<&'a str>,
    prefilter: Option<AhoCorasick>,
    remaining: usize,
}

impl<'a> SecretMatcher<'a> {
    fn new(secrets: Vec<&'a str>, limits: RedactionLimits) -> Result<Self> {
        ensure!(
            secrets.len() <= limits.patterns,
            ImportLimitError("redaction work")
        );
        let mut pattern_bytes = 0;
        for secret in &secrets {
            pattern_bytes = limited_add(
                pattern_bytes,
                secret.len(),
                limits.pattern_bytes,
                "redaction work",
            )?;
        }
        let remaining = limits
            .work
            .checked_sub(pattern_bytes)
            .ok_or(ImportLimitError("redaction work"))?;
        // Only ask whether an original string contains any secret. Leftmost
        // semantics avoid the standard automaton's inherited suffix-output
        // lists; its replacement order must never replace our priority rules.
        // Force an NFA so automatic DFA selection cannot multiply state space.
        let prefilter = if secrets.len() > 4 {
            Some(
                AhoCorasick::builder()
                    .kind(Some(AhoCorasickKind::NoncontiguousNFA))
                    .match_kind(MatchKind::LeftmostFirst)
                    .dense_depth(0)
                    .build(&secrets)
                    .map_err(|_| ImportLimitError("redaction work"))?,
            )
        } else {
            None
        };
        Ok(Self {
            secrets,
            prefilter,
            remaining,
        })
    }

    fn charge(&mut self, work: usize) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(work)
            .ok_or(ImportLimitError("redaction work"))?;
        Ok(())
    }
}

fn scrub_string(value: &str, matcher: &mut SecretMatcher<'_>) -> Result<String> {
    if matcher.secrets.is_empty() {
        return Ok(value.to_owned());
    }
    matcher.charge(value.len())?;
    if matcher
        .prefilter
        .as_ref()
        .is_some_and(|prefilter| !prefilter.is_match(value))
    {
        return Ok(value.to_owned());
    }
    // Only original text participates in later matches. Replacing into a
    // String repeatedly can match the marker itself and amplify tiny inputs.
    let mut fragments = vec![Some(value)];
    for pattern in 0..matcher.secrets.len() {
        matcher.charge(1)?;
        let secret = matcher.secrets[pattern];
        if secret.is_empty() || secret.len() > value.len() {
            continue;
        }
        // Charge all fragment visits, including markers with no text, before
        // reserving the next vector. A split string must not make later absent
        // secrets perform unbounded marker-only scans.
        matcher.charge(fragments.len())?;
        let mut next = Vec::with_capacity(fragments.len());
        for fragment in fragments {
            let Some(text) = fragment else {
                next.push(None);
                continue;
            };
            matcher.charge(text.len())?;
            matcher.charge(secret.len())?;
            let mut start = 0;
            for (index, _) in text.match_indices(secret) {
                matcher.charge(REDACTED.len() + 2)?;
                if start < index {
                    next.push(Some(&text[start..index]));
                }
                next.push(None);
                start = index + secret.len();
            }
            if start < text.len() {
                matcher.charge(1)?;
                next.push(Some(&text[start..]));
            }
        }
        fragments = next;
    }
    Ok(fragments
        .into_iter()
        .map(|fragment| fragment.unwrap_or(REDACTED))
        .collect())
}

fn scrub(
    value: &Value,
    options: &NormalizeOptions,
    matcher: &mut SecretMatcher<'_>,
) -> Result<Value> {
    Ok(match value {
        Value::String(s) => json!(scrub_string(s, matcher)?),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|v| scrub(v, options, matcher))
                .collect::<Result<_>>()?,
        ),
        Value::Object(values) => {
            let mut out = Map::new();
            for (key, value) in values {
                // Headers are not part of the retained event format, even if
                // embedded in provider extensions or imported records.
                if matches!(
                    key_name(key).as_str(),
                    "headers" | "requestheaders" | "responseheaders"
                ) {
                    continue;
                }
                let value = if sensitive_key(key, options) {
                    json!(REDACTED)
                } else {
                    scrub(value, options, matcher)?
                };
                out.insert(scrub_string(key, matcher)?, value);
            }
            Value::Object(out)
        }
        _ => value.clone(),
    })
}

fn ordered_unique_secrets(secrets: &[String]) -> Vec<&str> {
    // Preserve first-observed priority for equal-length overlapping secrets.
    // Deduplication after a length-only sort misses interleaved duplicates.
    let mut seen = BTreeSet::new();
    let mut unique: Vec<_> = secrets
        .iter()
        .map(String::as_str)
        .filter(|secret| !secret.is_empty() && seen.insert(*secret))
        .collect();
    unique.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    unique
}

fn with_secret_matcher(
    value: &Value,
    options: &NormalizeOptions,
    explicit: &[&str],
    remaining: &mut usize,
    redact: impl FnOnce(&mut SecretMatcher<'_>) -> Result<Value>,
) -> Result<Value> {
    let mut secrets = Vec::new();
    for secret in explicit {
        collect_secret_string(secret, &mut secrets);
    }
    collect_secrets(value, options, &mut secrets);
    let mut matcher = SecretMatcher::new(
        ordered_unique_secrets(&secrets),
        RedactionLimits {
            work: *remaining,
            ..REDACTION_LIMITS
        },
    )?;
    let result = redact(&mut matcher);
    *remaining = matcher.remaining;
    result
}

fn privacy(
    value: &Value,
    options: &NormalizeOptions,
    explicit: &[&str],
    remaining: &mut usize,
) -> Result<Value> {
    with_secret_matcher(value, options, explicit, remaining, |matcher| {
        scrub(value, options, matcher)
    })
}

fn privacy_capture_envelope(
    envelope: &Value,
    options: &NormalizeOptions,
    explicit: &[&str],
    remaining: &mut usize,
) -> Result<Value> {
    with_secret_matcher(envelope, options, explicit, remaining, |matcher| {
        let mut out = Map::new();
        // These root names are generated structure, not captured data. Scrubbing
        // "id" into a different key would make an otherwise valid capture
        // unsaveable. Original body keys and all metadata values still pass
        // through the ordinary privacy rules with one shared secret matcher.
        for (key, value) in envelope.as_object().expect("constructed capture envelope") {
            let value = if sensitive_key(key, options) {
                json!(REDACTED)
            } else {
                scrub(value, options, matcher)?
            };
            out.insert(key.clone(), value);
        }
        Ok(Value::Object(out))
    })
}

fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|k| (k.clone(), canonical(&map[k])))
                    .collect(),
            )
        }
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}

fn fingerprint(prefix: &str, value: &Value) -> String {
    let bytes = serde_json::to_vec(&canonical(value)).expect("JSON values serialize");
    format!("{prefix}_{:x}", Sha256::digest(bytes))
}

fn group_fingerprint(
    source: &Value,
    key: &Value,
    definition: &Value,
    presentation: &Value,
    task: &Value,
    isolation: &Value,
) -> String {
    fingerprint(
        "group_v1",
        &json!([source, key, definition, presentation, task, isolation]),
    )
}

fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

fn probability(value: &Value) -> Option<f64> {
    finite(value).filter(|n| (0.0..=1.0).contains(n))
}

fn token_count(value: &Value) -> Option<i64> {
    // SQLite's integer ledger is signed. A larger provider value must remain
    // unknown instead of disappearing at storage while still producing cost.
    value.as_i64().filter(|count| *count >= 0)
}

fn text_shape(value: &Value) -> bool {
    value.is_string() || value.is_object() || value.is_array()
}

fn extras(value: &Value, excluded: &[&str]) -> Value {
    value.as_object().map_or(Value::Null, |map| {
        Value::Object(
            map.iter()
                .filter(|(key, _)| !excluded.contains(&key.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    })
}

fn kind(definition: &Value, raw: &Value) -> &'static str {
    match definition
        .get("type")
        .or_else(|| raw.get("type"))
        .and_then(Value::as_str)
    {
        Some("choice") => "choice",
        Some("score") => "score",
        Some("noul") => "noul",
        _ => "unknown",
    }
}

fn validate_distribution(
    raw: &Value,
    expected: &[String],
    errors: &mut Vec<String>,
    profile: &Profile,
) -> Option<Vec<f64>> {
    let Some(map) = raw.get("probabilities").and_then(Value::as_object) else {
        errors.push("Missing probability distribution".into());
        return None;
    };
    if map.len() != expected.len() || expected.iter().any(|key| !map.contains_key(key)) {
        errors.push("Probability keys do not match the question criteria".into());
        return None;
    }
    let Some(values) = expected
        .iter()
        .map(|key| probability(&map[key]))
        .collect::<Option<Vec<_>>>()
    else {
        errors.push("Probabilities must be finite numbers between zero and one".into());
        return None;
    };
    // Only documented serialization precision changes this bound. Original
    // probabilities are retained; no renormalization or clipping is performed.
    let tolerance = profile
        .probability_decimals
        .map_or(PROBABILITY_TOLERANCE, |digits| {
            PROBABILITY_TOLERANCE.max(0.5 * 10_f64.powi(-digits) * values.len() as f64) + 1e-12
        });
    if (values.iter().sum::<f64>() - 1.0).abs() > tolerance {
        errors.push("Probabilities do not sum to one".into());
        return None;
    }
    Some(values)
}

fn validate_answer(
    definition: &Value,
    raw: &Value,
    kind: &str,
    profile: &Profile,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    if !definition.is_object() {
        errors.push("Original question definition unavailable".into());
    } else {
        if !matches!(
            definition["type"].as_str(),
            Some("noul" | "choice" | "score")
        ) {
            errors.push("Question type must be noul, choice, or score".into());
        }
        if !(text_shape(&definition["instructions"])
            || profile.json_text
            || (profile.optional_instructions && definition["instructions"].is_null()))
        {
            errors.push("Question instructions must be a string, object, or array".into());
        }
    }
    if !raw.is_object() {
        errors.push("Answer missing or not an object".into());
        return errors;
    }
    if raw["type"].as_str() != Some(kind) || kind == "unknown" {
        errors.push("Answer type is unknown or differs from the question".into());
    }
    match kind {
        "noul" => {
            if probability(&raw["noul"]).is_none() {
                errors.push("Noul must be a finite probability between zero and one".into());
            }
            if let Some(criteria) = definition.get("criteria") {
                let valid = criteria.as_object().is_some_and(|map| {
                    map.iter().all(|(key, value)| {
                        matches!(key.as_str(), "true" | "false")
                            && (text_shape(value) || profile.json_text)
                    })
                });
                if !valid {
                    errors.push("Invalid Noul criteria".into());
                }
            }
        }
        "choice" => {
            let list_criteria: Option<Map<String, Value>> = if profile.list_choice {
                definition["criteria"].as_array().and_then(|values| {
                    let entries = values
                        .iter()
                        .map(|value| value.as_str().map(|key| (key.to_owned(), Value::Null)))
                        .collect::<Option<Map<_, _>>>()?;
                    (entries.len() == values.len()).then_some(entries)
                })
            } else {
                None
            };
            let criteria = definition["criteria"]
                .as_object()
                .or(list_criteria.as_ref());
            if !criteria.is_some_and(|map| {
                !map.is_empty()
                    && map.len() <= profile.choice_max
                    && map
                        .values()
                        .all(|v| v.is_null() || text_shape(v) || profile.json_text)
            }) {
                errors.push(format!(
                    "Choice requires one to {} described or null options",
                    profile.choice_max
                ));
            }
            let expected: Vec<String> = criteria
                .map(|map| map.keys().cloned().collect())
                .unwrap_or_default();
            let choice = raw["choice"].as_str();
            if !choice.is_some_and(|c| expected.iter().any(|key| key == c)) {
                errors.push("Selected option is absent from the question criteria".into());
            }
            if let Some(probabilities) = validate_distribution(raw, &expected, &mut errors, profile)
                && let Some(selected) = choice.and_then(|c| expected.iter().position(|k| k == c))
                && probabilities
                    .iter()
                    .any(|p| *p > probabilities[selected] + PROBABILITY_TOLERANCE)
            {
                errors.push("Selected option is not a highest-probability option".into());
            }
            if profile.optional_confidence && raw.get("confidence").is_none() {
                warnings.push("Confidence was not reported; it remains unknown".into());
            } else if probability(&raw["confidence"]).is_none() {
                errors.push("Choice confidence is missing or outside zero to one".into());
            }
        }
        "score" => {
            let levels = definition["criteria"].as_array();
            let range = profile.score_min..=profile.score_max;
            if !levels.is_some_and(|v| {
                range.contains(&v.len())
                    && v.iter().all(|value| text_shape(value) || profile.json_text)
            }) {
                errors.push(format!(
                    "Score requires {} to {} ordered levels",
                    profile.score_min, profile.score_max
                ));
            }
            let n = levels.map_or(0, Vec::len);
            let score = finite(&raw["score"]);
            if !score
                .is_some_and(|s| range.contains(&n) && s >= 0.0 && s <= n.saturating_sub(1) as f64)
            {
                errors.push("Score must be between level zero and the last level index".into());
            }
            let expected: Vec<String> = (0..n).map(|n| n.to_string()).collect();
            if let Some(probabilities) = validate_distribution(raw, &expected, &mut errors, profile)
                && let Some(score) = score
            {
                let weighted: f64 = probabilities
                    .iter()
                    .enumerate()
                    .map(|(index, p)| index as f64 * p)
                    .sum();
                if profile.modal_score {
                    if score.fract() != 0.0
                        || !(0.0..probabilities.len() as f64).contains(&score)
                        || probabilities
                            .iter()
                            .any(|p| *p > probabilities[score as usize] + PROBABILITY_TOLERANCE)
                    {
                        errors.push(
                            "Modal Score must be a highest-probability integer level index".into(),
                        );
                    }
                    warnings.push("Score is the reported most likely level; the provider's expected value is retained in raw_answer.expected".into());
                } else if (weighted - score).abs() > 0.001 {
                    warnings.push("Reported Score differs from the weighted displayed probabilities; original values are preserved".into());
                }
            }
            if profile.optional_legend && raw.get("legend").is_none() {
                warnings.push("Score legend was not reported; inspect the original rubric".into());
            } else if !raw["legend"].as_object().is_some_and(|map| {
                map.len() == n
                    && expected.iter().all(|key| {
                        map.get(key).is_some_and(|value| {
                            value.is_string() || (profile.structured_legend && text_shape(value))
                        })
                    })
            }) {
                errors.push("Score legend must describe every level index".into());
            }
            if profile.optional_confidence && raw.get("confidence").is_none() {
                warnings.push("Confidence was not reported; it remains unknown".into());
            } else if probability(&raw["confidence"]).is_none() {
                errors.push("Score confidence is missing or outside zero to one".into());
            }
        }
        _ => {}
    }
    errors
}

fn jgrep_family(
    source: &str,
    key: &str,
    definition: &Value,
    task: &Value,
    adapter: Option<&str>,
) -> Option<Value> {
    if adapter != Some("jgrep-v1") || definition["type"] != "noul" {
        return None;
    }
    let index = key.strip_prefix('c')?;
    if index.is_empty()
        || !index.bytes().all(|b| b.is_ascii_digit())
        || (index.len() > 1 && index.starts_with('0'))
    {
        return None;
    }
    let instructions = definition["instructions"].as_str()?;
    let prefix = format!("Look only at the chunk with id \"{key}\". ");
    let remainder = instructions.strip_prefix(&prefix)?;
    let predicate = remainder.strip_prefix("Does that code match this description: ")
        .or_else(|| remainder.strip_prefix("Does that diff hunk (lines starting with + were added, - removed) match this description: "))?;
    if predicate.is_empty() {
        return None;
    }
    // Only this literal instance prefix is removed. Numbers, extra fields,
    // candidate descriptions and all remaining question text are preserved.
    let mut normalized = definition.clone();
    normalized["instructions"] = json!(remainder);
    let presentation = serde_json::to_string(&normalized).ok()?;
    Some(json!({
        "id": fingerprint("family_v1", &json!([source, "jgrep-v1", normalized, presentation, task])),
        "name": predicate,
        "instance_ref": key,
        "reason": "jgrep-v1: removed only the exact chunk-id instruction prefix; strict question identity retained"
    }))
}

/// Normalize a bounded copy. This never mutates or reconstructs the forwarded bytes.
pub fn normalize(capture: &Capture, options: &NormalizeOptions) -> Value {
    match normalize_with_secrets(capture, options, &[]) {
        Ok(record) => record,
        Err(_) => redaction_limited_capture(capture),
    }
}

fn redaction_limited_capture(capture: &Capture) -> Value {
    // Retain transport facts without cloning the captured bodies or copying
    // any original text into the fallback. A fresh identity cannot collapse
    // unrelated limited captures, even when their original IDs contained a
    // credential. Use the ordinary builder to keep its record schema intact.
    let safe_capture = Capture {
        id: format!("redacted_v1_{}", uuid::Uuid::new_v4().simple()),
        timestamp: capture.timestamp,
        source: REDACTED.into(),
        status: capture.status,
        duration_ms: capture.duration_ms,
        capture_complete: false,
        transport_error: capture.transport_error.as_ref().map(|_| REDACTED.into()),
        ..Default::default()
    };
    let request = json!({"questions": {}});
    let safe = json!({
        "request": request, "response": {"answers": {}}, "id": safe_capture.id,
        "source": safe_capture.source, "task_version": null,
        "transport_error": safe_capture.transport_error
    });
    let options = NormalizeOptions {
        provider: Some("unknown".into()),
        ..Default::default()
    };
    let mut record = normalize_prepared(&safe_capture, &options, &request, &safe);
    record["normalization_error"] = json!(REDACTION_LIMIT_MESSAGE);
    record
}

fn clear_input_fields(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        for field in INPUT_FIELDS {
            if let Some(input) = map.get_mut(field) {
                *input = Value::Null;
            }
        }
    }
}

fn normalize_with_secrets(
    capture: &Capture,
    options: &NormalizeOptions,
    import_secrets: &[String],
) -> Result<Value> {
    let mut remaining = REDACTION_LIMITS.work;
    let (raw_request, safe) = prepare_capture(capture, options, import_secrets, &mut remaining)?;
    Ok(normalize_prepared(capture, options, &raw_request, &safe))
}

fn normalize_import(
    capture: &Capture,
    options: &NormalizeOptions,
    import_secrets: &[String],
    budget: &mut ImportRecordBudget<'_>,
    charge_input: bool,
) -> Result<Value> {
    // Imported Observer envelopes may contain the additional wrappers our
    // own output adds. Never silently replace their reconstructed bodies with
    // null because a raw-provider parser has a smaller recursion allowance.
    let raw_request = crate::normalized_json::from_slice(&capture.request)?;
    let response = crate::normalized_json::from_slice(&capture.response)?;
    let (raw_request, safe) = prepare_values(
        capture,
        options,
        import_secrets,
        raw_request,
        response,
        &mut budget.batch.redaction_remaining,
    )?;
    if charge_input {
        budget.value(&safe)?;
    }
    // Use the sanitized lengths: short secrets can expand to redaction markers.
    // Charge before fingerprints and the answer Vec replicate this context.
    let source = safe["source"].as_str().unwrap_or("default");
    let context = limited_add(
        budget.size(source)?,
        budget.size(&safe["task_version"])?,
        budget.batch.limits.record,
        "normalization work",
    )?;
    let context_redacted = context_redacted(capture, &safe);
    let isolation = budget.size(&safe["id"])?;
    for key in answer_keys(&safe["request"], &safe["response"]) {
        budget.charge(context)?;
        if definition_redacted(&raw_request, &safe, key, context_redacted) {
            budget.charge(isolation)?;
        }
    }
    Ok(normalize_prepared(capture, options, &raw_request, &safe))
}

fn prepare_capture(
    capture: &Capture,
    options: &NormalizeOptions,
    import_secrets: &[String],
    remaining: &mut usize,
) -> Result<(Value, Value)> {
    let raw_request: Value = serde_json::from_slice(&capture.request).unwrap_or(Value::Null);
    let response: Value = serde_json::from_slice(&capture.response).unwrap_or(Value::Null);
    prepare_values(
        capture,
        options,
        import_secrets,
        raw_request,
        response,
        remaining,
    )
}

fn prepare_values(
    capture: &Capture,
    options: &NormalizeOptions,
    import_secrets: &[String],
    raw_request: Value,
    response: Value,
    remaining: &mut usize,
) -> Result<(Value, Value)> {
    let explicit_secrets: Vec<&str> = [
        capture.secret.as_deref(),
        capture.local_token.as_deref(),
        capture.access_token.as_deref(),
    ]
    .into_iter()
    .flatten()
    .chain(import_secrets.iter().map(String::as_str))
    .collect();
    let mut safe = {
        let mut envelope = json!({
            "request": raw_request, "response": null, "id": capture.id,
            "source": capture.source, "task_version": capture.task_version,
            "transport_error": capture.transport_error
        });
        envelope["response"] = response;
        privacy_capture_envelope(&envelope, options, &explicit_secrets, remaining)?
    };
    // Only the actual input snapshot is opt-in. A structured question can
    // legitimately contain a field named `state`; that is part of its rule.
    if !options.capture_state {
        for envelope in ["request", "response"] {
            clear_input_fields(&mut safe[envelope]);
        }
        // Earlier capture-decision converters nested response envelope inputs
        // here. Apply the same preference to those saved captures and exports,
        // without recursively removing fields from question/result semantics.
        if let Some(metadata) = safe["response"].get_mut("original_metadata") {
            clear_input_fields(metadata);
        }
    }
    Ok((raw_request, safe))
}

fn context_redacted(capture: &Capture, safe: &Value) -> bool {
    safe["source"]
        .as_str()
        .is_some_and(|source| source.contains(REDACTED))
        || safe["task_version"].to_string().contains(REDACTED)
        || safe["source"] != capture.source
        || safe["task_version"] != json!(capture.task_version)
}

fn definition_redacted(
    raw_request: &Value,
    safe: &Value,
    key: &str,
    context_redacted: bool,
) -> bool {
    let definition = &safe["request"]["questions"][key];
    context_redacted
        || key.contains(REDACTED)
        || raw_request["questions"]
            .get(key)
            .is_some_and(|original| original != definition)
        || definition.to_string().contains(REDACTED)
}

fn normalize_prepared(
    capture: &Capture,
    options: &NormalizeOptions,
    raw_request: &Value,
    safe: &Value,
) -> Value {
    let request = &safe["request"];
    let response = &safe["response"];
    let source = safe["source"].as_str().unwrap_or("default");
    let task = &safe["task_version"];
    let context_redacted = context_redacted(capture, safe);
    let questions = request["questions"].as_object();
    let raw_answers = response["answers"].as_object();
    let profile = catalog::profile(options.provider.as_deref().unwrap_or("typesafe"));
    let mut keys: Vec<String> = questions
        .map(|q| q.keys().cloned().collect())
        .unwrap_or_default();
    let mut seen: BTreeSet<String> = keys.iter().cloned().collect();
    if let Some(answers) = raw_answers {
        for key in answers.keys() {
            if seen.insert(key.clone()) {
                keys.push(key.clone());
            }
        }
    }
    let answers: Vec<Value> = keys.iter().map(|key| {
        let definition = questions.and_then(|q| q.get(key)).unwrap_or(&Value::Null);
        let raw = raw_answers.and_then(|a| a.get(key)).unwrap_or(&Value::Null);
        let kind = kind(definition, raw);
        let definition_id = fingerprint("def_v1", definition);
        let presentation_id = fingerprint("presentation_v1", &json!(serde_json::to_string(definition).expect("JSON value")));
        let definition_redacted = definition_redacted(raw_request, safe, key, context_redacted);
        // Hidden semantic differences must not collapse into one series.
        // Request isolation remains recomputable after redacted export.
        let isolation = if definition_redacted { safe["id"].clone() } else { Value::Null };
        let group_id = group_fingerprint(&json!(source), &json!(key), &json!(definition_id), &json!(presentation_id), task, &isolation);
        let candidate_id = if matches!(kind, "choice" | "score") && !definition["criteria"].is_null() {
            json!(fingerprint("candidates_v1", &definition["criteria"]))
        } else { Value::Null };
        let mut warnings = Vec::new();
        let mut errors = validate_answer(definition, raw, kind, &profile, &mut warnings);
        if !(200..300).contains(&capture.status) { errors.push("Request did not complete with a successful HTTP status".into()); }
        if capture.transport_error.is_some() { errors.push("Request had a transport error".into()); }
        if !capture.capture_complete { errors.push("Capture is incomplete".into()); }
        let value = match kind {
            "choice" => raw.get("choice").filter(|v| v.is_string()).cloned(),
            "score" => raw.get("score").filter(|v| v.is_number()).cloned(),
            "noul" => raw.get("noul").filter(|v| v.is_number()).cloned(),
            _ => None,
        }.unwrap_or(Value::Null);
        let family = if definition_redacted { Value::Null } else { jgrep_family(source, key, definition, task, capture.adapter.as_deref()).unwrap_or(Value::Null) };
        json!({
            "key": key, "kind": kind, "group_id": group_id,
            "definition_id": definition_id, "presentation_id": presentation_id,
            "candidate_id": candidate_id, "task_version": task,
            "definition": definition, "definition_redacted": definition_redacted, "value": value,
            "probabilities": raw.get("probabilities").filter(|v| v.is_object()),
            "confidence": if matches!(kind, "choice" | "score") { probability(&raw["confidence"]) } else { None },
            "valid": errors.is_empty(), "warnings": warnings, "error": if errors.is_empty() { None } else { Some(errors.join("; ")) },
            "family_id": family["id"], "family_name": family["name"],
            "adapter": if family.is_null() { Value::Null } else { json!("jgrep-v1") },
            "instance_ref": family["instance_ref"],
            "mapping_reason": if definition_redacted { json!("Definition or grouping context was redacted; equivalence cannot be established, so this observation is isolated and family pooling is disabled") } else { family["reason"].clone() },
            "raw_answer": raw
        })
    }).collect();
    let input_tokens = token_count(&response["usage"]["input_tokens"]);
    let output_tokens = token_count(&response["usage"]["output_tokens"]);
    let estimated_cost = match (
        input_tokens,
        output_tokens,
        options.input_price_per_million,
        options.output_price_per_million,
    ) {
        (Some(input), Some(output), Some(ir), Some(or))
            if ir.is_finite() && or.is_finite() && ir >= 0.0 && or >= 0.0 =>
        {
            let value = (input as f64 * ir + output as f64 * or) / 1_000_000.0;
            value.is_finite().then_some(value)
        }
        _ => None,
    };
    // Only this adapter has a verified USD meaning for `usage.cost`.
    // Custom upstreams can use the same field name for credits or other units.
    let reported_cost = (options.provider.as_deref() == Some("openrouter"))
        .then(|| finite(&response["usage"]["cost"]).filter(|value| *value >= 0.0))
        .flatten();
    let cost = reported_cost.or(estimated_cost);
    let cost_basis = if reported_cost.is_some() {
        Some("provider_reported")
    } else {
        estimated_cost.map(|_| "configured_estimate")
    };
    let normalization_error = if !request.is_object() {
        Some("Request body is not a complete JSON object")
    } else if !request["questions"].is_object() {
        Some("Request questions is not an object")
    } else if !response.is_object() {
        Some("Response body is not a complete JSON object")
    } else if (200..300).contains(&capture.status) && !response["answers"].is_object() {
        Some("Successful response has no answers object")
    } else {
        None
    };
    let mut record = json!({
        "schema_version": 1, "event_kind": "request", "id": safe["id"], "timestamp": capture.timestamp,
        "source": source, "provider": options.provider.as_deref().unwrap_or("typesafe"),
        "model": response.get("model").filter(|v| v.is_string()),
        "requested_model": request.get("model").filter(|v| v.is_string()),
        "status": if capture.status == 0 { None } else { Some(capture.status) },
        "duration_ms": if capture.duration_ms.is_finite() && capture.duration_ms >= 0.0 { Some(capture.duration_ms) } else { None },
        "input_tokens": input_tokens, "output_tokens": output_tokens,
        "cost_usd": cost, "cost_basis": cost_basis,
        "price": if cost_basis == Some("configured_estimate") { json!({"input_per_million": options.input_price_per_million, "output_per_million": options.output_price_per_million}) } else { Value::Null },
        "source_event_id": Value::Null, "import_format": Value::Null,
        "capture_complete": capture.capture_complete,
        "state_retained": options.capture_state && request.get("state").is_some_and(|v| !v.is_null()),
        "state": if options.capture_state { request.get("state").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "transport_error": safe["transport_error"], "sample": false,
        "answers": [], "actions": [], "labels": [],
        "normalization_error": normalization_error,
        "request_extra": extras(request, &["model", "state", "questions"]),
        "response_extra": extras(response, &["model", "answers", "usage"]),
        "usage_extra": extras(&response["usage"], &["input_tokens", "output_tokens"])
    });
    record["answers"] = Value::Array(answers);
    record
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .with_context(|| format!("{key} must be a nonempty string"))
}

fn nullable_number(value: &Value, key: &str) -> Result<()> {
    ensure!(
        value[key].is_null() || finite(&value[key]).is_some_and(|n| n >= 0.0),
        "{key} must be a nonnegative number or null"
    );
    Ok(())
}

fn charge_annotations(
    record: &Value,
    key: &str,
    source: &str,
    event_id: &str,
    budget: &mut ImportRecordBudget<'_>,
) -> Result<()> {
    let annotations = record[key]
        .as_array()
        .with_context(|| format!("{key} must be an array"))?;
    let source_bytes = budget.size(source)?;
    let event_bytes = budget.size(event_id)?;
    budget.value(&record[key])?;
    for annotation in annotations {
        if annotation.get("source").is_none() {
            budget.charge(source_bytes)?;
        }
        if annotation.get("source_event_id").is_none() {
            budget.charge(event_bytes)?;
        }
    }
    Ok(())
}

fn imported_annotations(record: &Value, key: &str, source: &str, event_id: &str) -> Result<Value> {
    let annotations = record[key]
        .as_array()
        .with_context(|| format!("{key} must be an array"))?;
    let mut out = Vec::new();
    for annotation in annotations {
        ensure!(annotation.is_object(), "{key} entries must be objects");
        if key == "labels" {
            annotation["key"].as_str().context("key must be a string")?;
            ensure!(
                matches!(
                    annotation["label"].as_str(),
                    Some("correct" | "incorrect" | "unknown")
                ),
                "Unsupported label value"
            );
        }
        let mut annotation = annotation.clone();
        let obj = annotation.as_object_mut().expect("checked object");
        obj.entry("source").or_insert_with(|| json!(source));
        obj.entry("source_event_id")
            .or_insert_with(|| json!(event_id));
        out.push(annotation);
    }
    Ok(json!(out))
}

fn remove_receipt_response_inputs(response: &mut Value) {
    let Some(map) = response.as_object_mut() else {
        return;
    };
    for field in INPUT_FIELDS {
        map.remove(field);
    }
    // Older converters kept envelope inputs in this metadata wrapper. Only
    // these known locations are inputs; answer semantics remain untouched.
    if let Some(metadata) = map
        .get_mut("original_metadata")
        .and_then(Value::as_object_mut)
    {
        for field in INPUT_FIELDS {
            metadata.remove(field);
        }
    }
}

fn remove_receipt_inputs(receipt: &mut Value) {
    let Some(map) = receipt.as_object_mut() else {
        return;
    };
    if let Some(decision) = map.get_mut("decision").and_then(Value::as_object_mut) {
        decision.remove("input");
    }
    for key in [
        "routing_input",
        "request",
        "state",
        "input",
        "context",
        "actor",
    ] {
        map.remove(key);
    }
    if let Some(raw) = map.get_mut("raw_jev") {
        remove_receipt_response_inputs(raw);
    }
    if let Some(stages) = map.get_mut("raw_jev_stages").and_then(Value::as_array_mut) {
        for stage in stages {
            if let Some(response) = stage.get_mut("response") {
                remove_receipt_response_inputs(response);
            }
        }
    }
}

fn import_observer(
    record: &Value,
    options: &NormalizeOptions,
    budget: &mut ImportRecordBudget<'_>,
) -> Result<Value> {
    ensure!(
        record["schema_version"] == 1,
        "Unsupported Observer schema_version"
    );
    for key in [
        "id",
        "timestamp",
        "source",
        "provider",
        "model",
        "requested_model",
        "status",
        "duration_ms",
        "input_tokens",
        "output_tokens",
        "cost_usd",
        "cost_basis",
        "source_event_id",
        "import_format",
        "capture_complete",
        "state_retained",
        "state",
        "transport_error",
        "sample",
        "answers",
        "actions",
        "labels",
    ] {
        ensure!(record.get(key).is_some(), "Missing Observer field: {key}");
    }
    let id = required_string(record, "id")?;
    let source = required_string(record, "source")?;
    for key in ["timestamp", "imported_at"] {
        ensure!(
            record[key].is_null() || record[key].as_i64().is_some_and(valid_timestamp),
            "{key} must be valid epoch milliseconds or null"
        );
    }
    for key in ["duration_ms", "cost_usd"] {
        nullable_number(record, key)?;
    }
    for key in ["input_tokens", "output_tokens"] {
        ensure!(
            record[key].is_null() || token_count(&record[key]).is_some(),
            "{key} must be an integer between zero and {} or null",
            i64::MAX
        );
    }
    ensure!(
        record["status"].is_null()
            || record["status"]
                .as_u64()
                .is_some_and(|n| (100..=599).contains(&n)),
        "status must be a valid HTTP status or null"
    );
    for key in ["capture_complete", "state_retained", "sample"] {
        ensure!(record[key].is_boolean(), "{key} must be boolean");
    }
    for key in [
        "provider",
        "model",
        "requested_model",
        "source_event_id",
        "import_format",
        "transport_error",
        "cost_basis",
    ] {
        ensure!(
            record[key].is_null() || record[key].is_string(),
            "{key} must be a string or null"
        );
    }
    let event_id = record["source_event_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(id);
    // Charge retained provenance and defaults before reconstructing question
    // maps. Derived export fields are not themselves normalization inputs.
    for key in [
        "provider",
        "cost_basis",
        "price",
        "receipt",
        "imported_at",
        "timestamp_basis",
        "observation_origin",
        "normalization_error",
    ] {
        budget.value(&record[key])?;
    }
    budget.charge(budget.size(event_id)?)?;
    for key in ["actions", "labels"] {
        charge_annotations(record, key, "observer-jsonl", event_id, budget)?;
    }
    let answers = record["answers"]
        .as_array()
        .context("answers must be an array")?;
    let mut questions = Map::new();
    let mut raw_answers = Map::new();
    let mut original_answers = BTreeMap::new();
    for answer in answers {
        // Question map keys are arbitrary strings, including the empty string.
        // Preserve the same key domain as normalization and label creation.
        let key = answer["key"].as_str().context("key must be a string")?;
        ensure!(
            original_answers.insert(key, answer).is_none(),
            "Duplicate answer key"
        );
        ensure!(
            answer.get("definition").is_some() && answer.get("raw_answer").is_some(),
            "Answer must retain definition and raw_answer (null allowed)"
        );
        // Captures retain malformed question values as evidence. Revalidation
        // below keeps them invalid while preserving their JSON and identity.
        if !answer["definition"].is_null() {
            questions.insert(key.to_owned(), answer["definition"].clone());
        }
        if !answer["raw_answer"].is_null() {
            raw_answers.insert(key.to_owned(), answer["raw_answer"].clone());
        }
        // Preserve missing-answer placeholders so a definition-less imported
        // observation is not silently discarded.
        if answer["definition"].is_null() && answer["raw_answer"].is_null() {
            questions.insert(key.to_owned(), Value::Null);
        }
    }
    let mut request = record
        .get("request_extra")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    request["model"] = record["requested_model"].clone();
    request["questions"] = Value::Object(questions);
    request["state"] = if record["state_retained"] == true {
        record["state"].clone()
    } else {
        Value::Null
    };
    let mut response = record
        .get("response_extra")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    response["model"] = record["model"].clone();
    response["answers"] = Value::Object(raw_answers);
    let mut usage = record
        .get("usage_extra")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or(json!({}));
    usage["input_tokens"] = record["input_tokens"].clone();
    usage["output_tokens"] = record["output_tokens"].clone();
    response["usage"] = usage;
    let task = answers
        .first()
        .and_then(|a| a["task_version"].as_str())
        .map(str::to_owned);
    ensure!(
        answers
            .iter()
            .all(|a| a["task_version"].as_str() == task.as_deref()),
        "Mixed task versions in one request"
    );
    let adapter = answers
        .iter()
        .any(|a| a["adapter"] == "jgrep-v1")
        .then(|| "jgrep-v1".to_owned());
    let capture = Capture {
        id: id.to_owned(),
        timestamp: record["timestamp"].as_i64().unwrap_or(0),
        source: source.to_owned(),
        task_version: task,
        adapter,
        status: record["status"].as_u64().unwrap_or(0) as u16,
        duration_ms: finite(&record["duration_ms"]).unwrap_or(f64::NAN),
        request: serde_json::to_vec(&request)?,
        response: serde_json::to_vec(&response)?,
        capture_complete: record["capture_complete"] == true,
        transport_error: record["transport_error"].as_str().map(str::to_owned),
        secret: None,
        local_token: None,
        access_token: None,
    };
    let mut imported_options = options.clone();
    imported_options.provider = record["provider"].as_str().map(str::to_owned);
    let mut out = normalize_import(&capture, &imported_options, &[], budget, true)?;
    for derived in out["answers"].as_array_mut().expect("normalized answers") {
        // The same index validates uniqueness and retains per-answer privacy
        // evidence without rescanning a potentially large export for each key.
        let original = derived["key"]
            .as_str()
            .and_then(|key| original_answers.get(key));
        // Removed fields leave no marker. A conservative isolation flag can
        // only deny pooling; supplied fingerprints remain untrusted.
        if original.is_some_and(|answer| answer["definition_redacted"] == true)
            && derived["definition_redacted"] != true
        {
            budget.charge(budget.size(source)?)?;
            budget.value(&derived["task_version"])?;
            budget.charge(budget.size(id)?)?;
            derived["definition_redacted"] = json!(true);
            derived["group_id"] = json!(group_fingerprint(
                &json!(source),
                &derived["key"],
                &derived["definition_id"],
                &derived["presentation_id"],
                &derived["task_version"],
                &json!(id)
            ));
            for key in ["family_id", "family_name", "adapter", "instance_ref"] {
                derived[key] = Value::Null;
            }
            derived["mapping_reason"] = json!(
                "Definition or grouping context was redacted; equivalence cannot be established, so this observation is isolated and family pooling is disabled"
            );
        }
    }
    if let Some(event_kind) = record.get("event_kind") {
        ensure!(
            matches!(event_kind.as_str(), Some("request" | "application_action")),
            "Unsupported event_kind"
        );
        out["event_kind"] = event_kind.clone();
        if event_kind == "application_action" {
            ensure!(
                answers.is_empty(),
                "Application action records cannot supply inference answers"
            );
        }
    }
    out["timestamp"] = record["timestamp"].clone();
    out["provider"] = record["provider"].clone();
    out["sample"] = record["sample"].clone();
    out["source_event_id"] = json!(event_id);
    out["import_format"] = json!("observer-jsonl");
    out["cost_usd"] = record["cost_usd"].clone();
    out["cost_basis"] = if record["cost_usd"].is_null() {
        Value::Null
    } else {
        ensure!(
            !record["cost_basis"].as_str().unwrap_or("").is_empty(),
            "Known cost requires its cost_basis"
        );
        record["cost_basis"].clone()
    };
    out["price"] = record.get("price").cloned().unwrap_or(Value::Null);
    out["actions"] = imported_annotations(record, "actions", "observer-jsonl", event_id)?;
    out["labels"] = imported_annotations(record, "labels", "observer-jsonl", event_id)?;
    // Keep additional provenance without treating supplied fingerprints,
    // validity flags or derived values as authoritative.
    for key in [
        "receipt",
        "imported_at",
        "timestamp_basis",
        "observation_origin",
        "normalization_error",
    ] {
        if let Some(value) = record.get(key) {
            out[key] = value.clone();
        }
    }
    if out["timestamp"].is_null() {
        if out["imported_at"].is_null() {
            out["imported_at"] = json!(chrono::Utc::now().timestamp_millis());
        }
        out["timestamp_basis"] = json!("import");
    }
    if !options.capture_state {
        if let Some(receipt) = out.get_mut("receipt") {
            remove_receipt_inputs(receipt);
        }
        for action in out["actions"]
            .as_array_mut()
            .expect("validated annotations")
        {
            remove_receipt_inputs(action);
        }
    }
    privacy(&out, options, &[], &mut budget.batch.redaction_remaining)
}

fn import_receipt(
    record: &Value,
    options: &NormalizeOptions,
    budget: &mut ImportRecordBudget<'_>,
) -> Result<Value> {
    let decision_id = required_string(record, "decision_id")?;
    required_string(record, "request_id")?;
    ensure!(
        record["mode"] == "decision_only",
        "Only JevRouter decision receipts are supported (plans are separate records)"
    );
    ensure!(
        matches!(
            record["status"].as_str(),
            Some("selected" | "needs_confirmation" | "no_decision")
        ),
        "Invalid JevRouter decision status"
    );
    ensure!(
        record["decision"]["kind"] == "choice" && record["decision"]["candidates"].is_array(),
        "Invalid JevRouter decision"
    );
    required_string(&record["provenance"], "jev_provider")?;
    ensure!(
        record["execution"].is_object() && record["fallback"].is_object(),
        "Missing receipt execution/fallback provenance"
    );
    ensure!(
        record["raw_jev"].is_null() || record["raw_jev"].is_object(),
        "raw_jev must be an object or null"
    );
    let source = "jevrouter";
    let provider = &record["provenance"]["jev_provider"];
    // The original receipt and its action retain overlapping provenance. Charge
    // the second copy before cloning, including discarded input fields (a safe
    // conservative bound), then charge envelope extensions retained a third time.
    budget.value(record)?;
    if let Some(raw) = record["raw_jev"].as_object() {
        for (key, value) in raw {
            if !matches!(key.as_str(), "model" | "answers" | "usage") {
                budget.charge(budget.size(key)?)?;
                budget.value(value)?;
            }
        }
    }
    let response = record
        .get("raw_jev")
        .and_then(Value::as_object)
        .map(|raw| {
            Value::Object(
                raw.iter()
                    .filter(|(key, _)| key.as_str() != "answers")
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        })
        .unwrap_or(json!({}));
    let capture = Capture {
        id: fingerprint("receipt", &json!([source, decision_id])),
        source: source.into(),
        request: serde_json::to_vec(&json!({"questions": {}}))?,
        response: serde_json::to_vec(&response)?,
        duration_ms: f64::NAN,
        // A receipt is not a captured HTTP transaction. Do not fabricate its
        // status, original definitions, model call, or capture completeness.
        ..Capture::default()
    };
    // Receipt answers are provenance, not inference observations. They were
    // already included in the global privacy pass and are preserved below.
    let mut out = normalize_import(&capture, options, &[], budget, false)?;
    out["timestamp"] = Value::Null;
    out["timestamp_basis"] = json!("import");
    out["imported_at"] = json!(chrono::Utc::now().timestamp_millis());
    out["provider"] = provider.clone();
    out["source_event_id"] = json!(decision_id);
    out["import_format"] = json!("jevrouter-receipt");
    out["observation_origin"] = json!("imported_receipt_not_verified_live_inference");
    out["event_kind"] = json!("application_action");
    out["answers"] = json!([]);
    out["input_tokens"] = Value::Null;
    out["output_tokens"] = Value::Null;
    out["normalization_error"] = Value::Null;
    out["sample"] = provider
        .as_str()
        .is_some_and(|p| p == "demo" || p == "jevrouter-demo")
        .into();
    out["cost_usd"] = Value::Null;
    out["cost_basis"] = Value::Null;
    out["price"] = Value::Null;
    out["actions"] = json!([{
        "kind": "policy_decision", "key": "tool", "source": "jevrouter-receipt",
        "source_event_id": decision_id, "status": record["status"],
        "decision": record["decision"], "execution": record["execution"],
        "fallback": record["fallback"], "provenance": record["provenance"],
        "raw_jev": record["raw_jev"], "raw_jev_stages": record["raw_jev_stages"]
    }]);
    let mut receipt = record.clone();
    // Inputs are application state too, even though this format uses a
    // different field name. The remaining receipt is retained as provenance.
    if !options.capture_state {
        remove_receipt_inputs(&mut receipt);
        remove_receipt_inputs(&mut out["actions"][0]);
    }
    out["receipt"] = receipt;
    privacy(&out, options, &[], &mut budget.batch.redaction_remaining)
}

/// A raw, explicitly identified call, including mapped library decisions. The
/// envelope records observations; supplied validity, cost and group IDs are ignored.
fn import_capture(
    record: &Value,
    options: &NormalizeOptions,
    budget: &mut ImportRecordBudget<'_>,
) -> Result<Value> {
    let event_id = required_string(record, "id")?;
    let source = required_string(record, "source")?;
    let provider = required_string(record, "provider")?;
    ensure!(
        provider.len() <= 64
            && provider
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')),
        "Invalid provider name"
    );
    let timestamp = record["timestamp"]
        .as_i64()
        .filter(|timestamp| valid_timestamp(*timestamp))
        .context("timestamp must be valid epoch milliseconds")?;
    let status = record["status"]
        .as_u64()
        .filter(|status| (100..=599).contains(status))
        .context("status must be an HTTP status from 100 to 599")?;
    let complete = record["capture_complete"]
        .as_bool()
        .context("capture_complete must be a boolean")?;
    ensure!(
        record["request"].is_object() && record["request"]["questions"].is_object(),
        "request must contain named System One questions"
    );
    ensure!(record["response"].is_object(), "response must be an object");
    for key in ["transport_error", "task_version"] {
        ensure!(
            record[key].is_null() || record[key].is_string(),
            "{key} must be a string or null"
        );
    }
    nullable_number(record, "duration_ms")?;
    let id = fingerprint(
        "capture_v1",
        &json!([source, provider.to_ascii_lowercase(), event_id]),
    );
    let capture = Capture {
        id,
        timestamp,
        source: source.to_owned(),
        task_version: record["task_version"].as_str().map(str::to_owned),
        status: status as u16,
        duration_ms: finite(&record["duration_ms"]).unwrap_or(f64::NAN),
        request: serde_json::to_vec(&record["request"])?,
        response: serde_json::to_vec(&record["response"])?,
        capture_complete: complete,
        transport_error: record["transport_error"].as_str().map(str::to_owned),
        ..Default::default()
    };
    let mut imported_options = options.clone();
    imported_options.provider = Some(provider.to_ascii_lowercase());
    let mut import_secrets = Vec::new();
    collect_secrets(record, options, &mut import_secrets);
    let mut out = normalize_import(&capture, &imported_options, &import_secrets, budget, true)?;
    // Use the sanitized ID for deduplication, without retaining unsanitized
    // caller metadata. Export/reimport preserves this identity.
    out["source_event_id"] = out["id"].clone();
    out["import_format"] = json!("systemone-capture");
    ensure!(
        record["sample"].is_null() || record["sample"].is_boolean(),
        "sample must be a boolean or null"
    );
    out["sample"] = json!(record["sample"] == true);
    Ok(out)
}

/// Import is all-or-nothing validation. Storage owns deduplication by explicit
/// source event identity; equal payloads are never presumed to be the same call.
pub fn import_records(text: &str, format: &str, options: &NormalizeOptions) -> Result<Vec<Value>> {
    import_records_with_limits(text, format, options, IMPORT_LIMITS)
}

fn import_records_with_limits(
    text: &str,
    format: &str,
    options: &NormalizeOptions,
    limits: ImportLimits,
) -> Result<Vec<Value>> {
    ensure!(
        matches!(
            format,
            "observer-jsonl" | "jevrouter-receipt" | "systemone-capture"
        ),
        "Unsupported import format: {format}"
    );
    let mut values = Vec::new();
    if format == "jevrouter-receipt" {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            match value {
                Value::Array(items) => values = items,
                Value::Object(_) => values.push(value),
                _ => bail!("Receipt must be a JSON object or array"),
            }
        } else {
            for (index, line) in text
                .lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
            {
                ensure!(
                    values.len() < MAX_IMPORT_RECORDS,
                    ImportLimitError("record count")
                );
                values.push(
                    serde_json::from_str(line)
                        .with_context(|| format!("Invalid JSON at line {}", index + 1))?,
                );
            }
        }
    } else {
        for (index, line) in text
            .lines()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
        {
            ensure!(
                values.len() < MAX_IMPORT_RECORDS,
                ImportLimitError("record count")
            );
            let record = if format == "observer-jsonl" {
                crate::normalized_json::from_slice(line.as_bytes())
            } else {
                serde_json::from_str(line)
            };
            values.push(record.with_context(|| format!("Invalid JSON at line {}", index + 1))?);
        }
    }
    ensure!(!values.is_empty(), "Import contains no records");
    ensure!(
        values.len() <= MAX_IMPORT_RECORDS,
        ImportLimitError("record count")
    );
    let mut batch = ImportBudget {
        limits,
        work: 0,
        normalized: 0,
        redaction_remaining: REDACTION_LIMITS.work,
    };
    values
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let mut budget = ImportRecordBudget {
                batch: &mut batch,
                work: 0,
            };
            budget.structure(record, format)?;
            if format == "systemone-capture" {
                // normalize must see the original definition to detect redaction
                // and isolate hidden semantic changes before fingerprints are made.
                let out = import_capture(record, options, &mut budget)
                    .with_context(|| format!("Invalid record {}", index + 1))?;
                budget.finish(&out)?;
                return Ok(out);
            }
            // Scrub before normalization so all recomputed identities describe the
            // retained definition, and credential echoes cannot leak into metadata.
            let mut safe = privacy(record, options, &[], &mut budget.batch.redaction_remaining)?;
            // Before reconstruction/cloning, bound even privacy-expanded input.
            // Observer derived answer fields are not retained wholesale: its
            // reconstructed input and provenance are charged by import_observer.
            budget.size(&safe)?;
            if format == "jevrouter-receipt" {
                budget.value(&safe)?;
            }
            if format == "observer-jsonl"
                && let (Some(original), Some(retained)) = (
                    record.get("answers").and_then(Value::as_array),
                    safe.get_mut("answers").and_then(Value::as_array_mut),
                )
            {
                // Removed headers leave no marker behind. Preserve evidence of
                // any changed definition before the unsanitized record is lost;
                // imported validity/grouping claims cannot make it safe to pool.
                for (original, retained) in original.iter().zip(retained) {
                    if original.get("definition") != retained.get("definition")
                        && let Some(answer) = retained.as_object_mut()
                    {
                        answer.insert("definition_redacted".into(), json!(true));
                    }
                }
            }
            let result = if format == "observer-jsonl" {
                import_observer(&safe, options, &mut budget)
            } else {
                import_receipt(&safe, options, &mut budget)
            };
            let out = result.with_context(|| format!("Invalid record {}", index + 1))?;
            budget.finish(&out)?;
            Ok(out)
        })
        .collect()
}

/// A reproducible synthetic workload, not evidence of latency or model quality.
pub fn sample_records() -> Vec<Value> {
    let now = chrono::Utc::now().timestamp_millis();
    let options = NormalizeOptions {
        input_price_per_million: Some(0.042),
        output_price_per_million: Some(0.0),
        ..NormalizeOptions::default()
    };
    (0..720).map(|i| {
        let version = if i < 470 { 1 } else { 2 };
        let indexed = i % 9 == 0;
        let source = if indexed { "Code search" } else if i % 4 == 0 { "Commit checks" } else { "Support inbox" };
        let department = if i % 10 < 5 { "billing" } else if i % 10 < 8 { "technical" } else { "sales" };
        let mut probabilities = json!({"billing":0.1,"technical":0.1,"sales":0.1});
        probabilities[department] = json!(0.8);
        let urgency = ((i * 17 % 94) as f64 + 3.0) / 100.0;
        let level = (i % 3) as usize;
        let mut score_probabilities = json!({"0":0.1,"1":0.1,"2":0.1});
        score_probabilities[level.to_string()] = json!(0.8);
        let score: f64 = (0..3).map(|n| n as f64 * score_probabilities[n.to_string()].as_f64().unwrap()).sum();
        let mut questions = json!({
            "support_routing": {"type":"choice","instructions": if version == 1 { "Which team should handle this ticket?" } else { "Which team should handle this ticket? Route outages to technical even when payment is mentioned." },"criteria":{"billing":"Payments and refunds","technical":"Bugs and outages","sales":"Plans and upgrades"}},
            "urgency": {"type":"noul","instructions":"Does the customer need a response today?"},
            "frustration": {"type":"score","instructions":"How frustrated is the customer?","criteria":["Calm","Frustrated","Very angry"]}
        });
        let mut answers = json!({
            "support_routing":{"type":"choice","choice":department,"probabilities":probabilities,"confidence":0.72},
            "urgency":{"type":"noul","noul":urgency},
            "frustration":{"type":"score","score":score,"probabilities":score_probabilities,"legend":{"0":"Calm","1":"Frustrated","2":"Very angry"},"confidence":0.72}
        });
        if source == "Commit checks" {
            questions = json!({"breaking_change":{"type":"noul","instructions":"Does this commit change a public API?"},"review_priority":{"type":"score","instructions":"How much review does this change need?","criteria":["Routine","Careful review","Specialist review"]}});
            answers = json!({"breaking_change":{"type":"noul","noul":urgency},"review_priority":{"type":"score","score":score,"probabilities":score_probabilities,"legend":{"0":"Routine","1":"Careful review","2":"Specialist review"},"confidence":0.72}});
        }
        if indexed {
            questions = json!({}); answers = json!({});
            for n in 0..4 {
                let key = format!("c{n}");
                questions[&key] = json!({"type":"noul","instructions":format!("Look only at the chunk with id \"{key}\". Does that code match this description: handles retries with a limit of {}", if version == 1 { 3 } else { 5 })});
                answers[&key] = json!({"type":"noul","noul":((i + n * 13) % 100) as f64 / 100.0});
            }
        }
        let failed = i % 31 == 0;
        let response = if failed { json!({"error":{"message":"Synthetic upstream overload","type":"overloaded"}}) } else {
            json!({"model":if i % 6 == 0 { "jev-synthetic-b" } else { "jev-synthetic-a" },"answers":answers,"usage":{"input_tokens":240 + i % 1700,"output_tokens":24 + i % 40}})
        };
        let capture = Capture {
            id: format!("synthetic-{i:04}"), timestamp: now - (720 - i) as i64 * 110_000,
            source: source.into(), adapter: indexed.then(|| "jgrep-v1".into()),
            status: if failed { 529 } else { 200 },
            duration_ms: (45 + i * 37 % 380 + if i % 43 == 0 { 430 } else { 0 }) as f64,
            request: serde_json::to_vec(&json!({"model":"jev-synthetic","state":format!("Synthetic example {i}"),"questions":questions})).unwrap(),
            response: serde_json::to_vec(&response).unwrap(), capture_complete: true, ..Capture::default()
        };
        let mut record = normalize(&capture, &options);
        record["sample"] = json!(true);
        if !record["cost_usd"].is_null() { record["cost_basis"] = json!("synthetic"); }
        if !failed && !indexed && i % 13 == 0 {
            let key = record["answers"][0]["key"].clone();
            record["labels"] = json!([{"key":key,"label":if i % 39 == 0 {"incorrect"} else {"correct"},"source":"synthetic_fixture","timestamp":capture.timestamp}]);
        }
        record
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrub_test_string(value: &str, secrets: &[&str]) -> String {
        let mut matcher = SecretMatcher::new(secrets.to_vec(), REDACTION_LIMITS).unwrap();
        scrub_string(value, &mut matcher).unwrap()
    }

    #[test]
    fn provider_score_discrepancy_is_preserved_with_a_warning() {
        // Captured from OpenRouter's Jev response on 2026-10-02.
        let request = json!({"questions":{"q":{"type":"score","instructions":"Rate","criteria":["Calm","Frustrated","Very angry"]}}});
        let answer = json!({"type":"score","score":0.87,"confidence":0.79,"probabilities":{"0":0.14,"1":0.86,"2":0.0},"legend":{"0":"Calm","1":"Frustrated","2":"Very angry"}});
        let record = normalized(&capture(
            request.clone(),
            json!({"answers":{"q":answer.clone()}}),
        ));
        assert_eq!(record["answers"][0]["valid"], true);
        assert_eq!(record["answers"][0]["value"], 0.87);
        assert_eq!(record["answers"][0]["raw_answer"], answer);
        assert_eq!(
            record["answers"][0]["warnings"].as_array().unwrap().len(),
            1
        );
        let imported = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(
            imported[0]["answers"][0]["warnings"],
            record["answers"][0]["warnings"]
        );
        for invalid in [json!(-0.01), json!(3.0), json!("0.87")] {
            let mut bad = answer.clone();
            bad["score"] = invalid;
            assert_eq!(
                normalized(&capture(request.clone(), json!({"answers":{"q":bad}})))["answers"][0]["valid"],
                false
            );
        }
    }

    #[test]
    fn reported_cost_takes_precedence_and_keeps_its_basis_across_import() {
        let options = NormalizeOptions {
            provider: Some("openrouter".into()),
            input_price_per_million: Some(100.0),
            output_price_per_million: Some(100.0),
            ..Default::default()
        };
        let mut c = noul();
        let mut response: Value = serde_json::from_slice(&c.response).unwrap();
        response["usage"] = json!({"input_tokens":408,"output_tokens":71,"cost":0.000017136});
        c.response = serde_json::to_vec(&response).unwrap();
        let record = normalize(&c, &options);
        assert_eq!(record["provider"], "openrouter");
        assert_eq!(record["cost_usd"], 0.000017136);
        assert_eq!(record["cost_basis"], "provider_reported");
        assert!(record["price"].is_null());
        let imported = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(imported[0]["cost_usd"], record["cost_usd"]);
        assert_eq!(imported[0]["cost_basis"], record["cost_basis"]);
        for bad in [json!(-1.0), json!("0.1"), json!(true), Value::Null] {
            response["usage"]["cost"] = bad;
            c.response = serde_json::to_vec(&response).unwrap();
            assert_eq!(normalize(&c, &options)["cost_basis"], "configured_estimate");
            assert!(normalize(&c, &NormalizeOptions::default())["cost_usd"].is_null());
        }
        response["usage"] = json!({"cost":0.0});
        c.response = serde_json::to_vec(&response).unwrap();
        let free = normalize(&c, &options);
        assert_eq!(free["cost_usd"], 0.0);
        assert_eq!(free["cost_basis"], "provider_reported");
        assert!(free["input_tokens"].is_null());
    }

    #[test]
    fn unrecognized_cost_units_stay_extra_and_do_not_replace_configured_usd_estimates() {
        let mut c = noul();
        let mut response: Value = serde_json::from_slice(&c.response).unwrap();
        response["usage"] = json!({"input_tokens":100,"output_tokens":10,"cost":17.0});
        c.response = serde_json::to_vec(&response).unwrap();
        for provider in [None, Some("typesafe"), Some("laya"), Some("custom")] {
            let mut options = NormalizeOptions {
                provider: provider.map(str::to_owned),
                ..Default::default()
            };
            let unknown = normalize(&c, &options);
            assert!(unknown["cost_usd"].is_null());
            assert!(unknown["cost_basis"].is_null());
            assert_eq!(unknown["usage_extra"]["cost"], 17.0);
            options.input_price_per_million = Some(10.0);
            options.output_price_per_million = Some(20.0);
            let estimated = normalize(&c, &options);
            assert_eq!(estimated["cost_usd"], 0.0012);
            assert_eq!(estimated["cost_basis"], "configured_estimate");
            assert_eq!(estimated["usage_extra"]["cost"], 17.0);
        }
    }

    #[test]
    fn laya_rounded_distributions_and_extended_levels_roundtrip() {
        let options = NormalizeOptions {
            provider: Some("laya".into()),
            ..Default::default()
        };
        let levels: Vec<String> = (0..16).map(|n| format!("Level {n}")).collect();
        let probabilities: Map<String, Value> = (0..16)
            .map(|n| (n.to_string(), json!(if n == 0 { 0.0624 } else { 0.0625 })))
            .collect();
        let legend: Map<String, Value> = levels
            .iter()
            .enumerate()
            .map(|(n, label)| (n.to_string(), json!(label)))
            .collect();
        let c = capture(
            json!({"model":"english","questions":{"q":{"type":"score","instructions":"Rate","criteria":levels},"tag":{"type":"choice","instructions":"Choose","criteria":["a","b"]}}}),
            json!({"model":"laya-rl-agent","answers":{"q":{"type":"score","score":7.5,"confidence":0.0,"probabilities":probabilities,"legend":legend},"tag":{"type":"choice","choice":"a","probabilities":{"a":0.8,"b":0.2},"confidence":0.3,"answer_confidence":0.8}},"usage":{"input_tokens":42,"output_tokens":0,"truncated":false}}),
        );
        let record = normalize(&c, &options);
        assert!(
            record["answers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|answer| answer["valid"] == true)
        );
        assert_eq!(record["provider"], "laya");
        assert_eq!(record["output_tokens"], 0);
        assert!(record["cost_usd"].is_null());
        assert_eq!(record["usage_extra"]["truncated"], false);
        let imported = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert!(
            imported[0]["answers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|answer| answer["valid"] == true)
        );
        assert_eq!(
            imported[0]["answers"][1]["raw_answer"]["answer_confidence"],
            0.8
        );
    }

    fn capture(request: Value, response: Value) -> Capture {
        Capture {
            id: "request-1".into(),
            timestamp: 1_800_000_000_000,
            source: "test".into(),
            status: 200,
            duration_ms: 12.0,
            capture_complete: true,
            request: serde_json::to_vec(&request).unwrap(),
            response: serde_json::to_vec(&response).unwrap(),
            ..Capture::default()
        }
    }

    fn noul() -> Capture {
        capture(
            json!({"model":"jev-latest","state":"example","questions":{"urgent":{"type":"noul","instructions":"Urgent?"}}}),
            json!({"model":"jev-1.13","answers":{"urgent":{"type":"noul","noul":0.8}},"usage":{"input_tokens":100,"output_tokens":10}}),
        )
    }

    fn normalized(c: &Capture) -> Value {
        normalize(c, &NormalizeOptions::default())
    }

    #[test]
    fn imported_timestamps_share_the_dashboard_date_domain() {
        let capture = noul();
        let observer = normalized(&capture);
        let raw = json!({
            "id":"time-domain", "source":"time-domain", "provider":"typesafe",
            "timestamp":capture.timestamp, "status":200, "capture_complete":true,
            "request":serde_json::from_slice::<Value>(&capture.request).unwrap(),
            "response":serde_json::from_slice::<Value>(&capture.response).unwrap()
        });
        for (format, mut record, key) in [
            ("observer-jsonl", observer.clone(), "timestamp"),
            ("observer-jsonl", observer, "imported_at"),
            ("systemone-capture", raw, "timestamp"),
        ] {
            for timestamp in [-8_640_000_000_000_000_i64, -1, 0, 1, 8_640_000_000_000_000] {
                record[key] = json!(timestamp);
                let imported =
                    import_records(&record.to_string(), format, &NormalizeOptions::default())
                        .unwrap();
                assert_eq!(imported[0][key], timestamp, "{format} {key}");
            }
            for invalid in [
                json!(-8_640_000_000_000_001_i64),
                json!(8_640_000_000_000_001_i64),
                json!(i64::MIN),
                json!(i64::MAX),
                json!(u64::MAX),
                json!(1.5),
                json!(true),
                json!("1700000000000"),
            ] {
                record[key] = invalid;
                let error =
                    import_records(&record.to_string(), format, &NormalizeOptions::default())
                        .unwrap_err();
                assert!(format!("{error:#}").contains(key), "{format}: {error:#}");
                assert!(!error.is::<ImportLimitError>());
            }
        }
    }

    #[test]
    fn mixed_answers_count_cost_once_and_score_uses_level_indices() {
        let c = capture(
            json!({"questions":{
                "route":{"type":"choice","instructions":"Team?","criteria":{"a":null,"b":"Other"}},
                "priority":{"type":"score","instructions":"Rate","criteria":["Low","Medium","High"]},
                "urgent":{"type":"noul","instructions":"Urgent?"}
            }}),
            json!({"answers":{
            "route":{"type":"choice","choice":"b","probabilities":{"a":0.2,"b":0.8},"confidence":0.7},
            "priority":{"type":"score","score":1.6,"probabilities":{"0":0.1,"1":0.2,"2":0.7},"confidence":0.6,"legend":{"0":"Low","1":"Medium","2":"High"}},
            "urgent":{"type":"noul","noul":0.9,"confidence":0.99}
        },"usage":{"input_tokens":1_000_000,"output_tokens":50}}),
        );
        let out = normalize(
            &c,
            &NormalizeOptions {
                input_price_per_million: Some(2.0),
                output_price_per_million: Some(0.0),
                ..Default::default()
            },
        );
        assert_eq!(out["answers"].as_array().unwrap().len(), 3);
        assert!(
            out["answers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["valid"] == true),
            "{out}"
        );
        assert_eq!(out["cost_usd"], 2.0);
        assert!(out["answers"][2]["confidence"].is_null());
        assert!(out["answers"][0].get("cost_usd").is_none());
    }

    #[test]
    fn invalid_answers_are_preserved_but_excluded() {
        for bad in [
            json!({"type":"noul","noul":true}),
            json!({"type":"noul","noul":1.01}),
            json!({"type":"choice","noul":0.5}),
            Value::Null,
        ] {
            let mut c = noul();
            c.response = serde_json::to_vec(&json!({"answers":{"urgent":bad}})).unwrap();
            let out = normalized(&c);
            assert_eq!(out["answers"][0]["valid"], false);
            assert_eq!(out["answers"][0]["raw_answer"], bad);
        }
        let mut c = noul();
        c.status = 529;
        assert_eq!(normalized(&c)["answers"][0]["valid"], false);
        c.status = 200;
        c.capture_complete = false;
        assert_eq!(normalized(&c)["answers"][0]["valid"], false);
        c.capture_complete = true;
        c.transport_error = Some("connection reset".into());
        assert_eq!(normalized(&c)["answers"][0]["valid"], false);
    }

    #[test]
    fn answers_cannot_supply_the_missing_question_type() {
        for (mut definition, raw) in [
            (
                json!({"type":"noul","instructions":"Urgent?"}),
                json!({"type":"noul","noul":0.8}),
            ),
            (
                json!({"type":"choice","instructions":"Pick","criteria":{"a":null,"b":null}}),
                json!({"type":"choice","choice":"a","probabilities":{"a":0.8,"b":0.2},"confidence":0.7}),
            ),
            (
                json!({"type":"score","instructions":"Rate","criteria":["Low","High"]}),
                json!({"type":"score","score":0.8,"probabilities":{"0":0.2,"1":0.8},"legend":{"0":"Low","1":"High"},"confidence":0.7}),
            ),
        ] {
            let mut c = capture(
                json!({"questions":{"q":definition}}),
                json!({"answers":{"q":raw}}),
            );
            assert_eq!(normalized(&c)["answers"][0]["valid"], true);
            definition.as_object_mut().unwrap().remove("type");
            c.request = serde_json::to_vec(&json!({"questions":{"q":definition}})).unwrap();
            let mut record = normalized(&c);
            assert_eq!(record["answers"][0]["valid"], false, "{record}");
            assert_eq!(record["answers"][0]["raw_answer"], raw);
            assert!(
                record["answers"][0]["error"]
                    .as_str()
                    .unwrap()
                    .contains("Question type")
            );
            record["answers"][0]["valid"] = json!(true);
            let imported = import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default(),
            )
            .unwrap();
            assert_eq!(imported[0]["answers"][0]["valid"], false);
        }
    }

    #[test]
    fn distributions_and_score_consistency_are_checked() {
        let request = json!({"questions":{"q":{"type":"score","instructions":"Rate","criteria":["Low","Medium","High"]}}});
        let valid = json!({"type":"score","score":1.0,"probabilities":{"0":0.0,"1":1.0,"2":0.0},"legend":{"0":"Low","1":"Medium","2":"High"},"confidence":1.0});
        for (field, bad) in [
            ("score", json!(3.0)),
            ("confidence", json!(-0.1)),
            ("legend", json!({"1":"Low","2":"Medium","3":"High"})),
            ("probabilities", json!({"0":0.2,"1":0.2,"2":0.2})),
            ("probabilities", json!({"0":0.0,"1":1.0,"other":0.0})),
        ] {
            let mut raw = valid.clone();
            raw[field] = bad;
            assert_eq!(
                normalized(&capture(request.clone(), json!({"answers":{"q":raw}})))["answers"][0]["valid"],
                false
            );
        }
        let choice = capture(
            json!({"questions":{"q":{"type":"choice","instructions":"Pick","criteria":{"a":null,"b":null}}}}),
            json!({"answers":{"q":{"type":"choice","choice":"a","probabilities":{"a":0.1,"b":0.9},"confidence":0.8}}}),
        );
        assert_eq!(normalized(&choice)["answers"][0]["valid"], false);
    }

    #[test]
    fn unknowns_are_not_zero_or_requested_model() {
        let mut c = noul();
        c.response = b"{\"answers\":{}}".to_vec();
        let out = normalized(&c);
        for key in [
            "input_tokens",
            "output_tokens",
            "cost_usd",
            "cost_basis",
            "model",
        ] {
            assert!(out[key].is_null());
        }
        assert!(out["answers"][0]["value"].is_null());
        assert_eq!(out["answers"][0]["valid"], false);
        c.request = b"{bad".to_vec();
        c.response = b"not-json-secret".to_vec();
        let out = normalized(&c);
        assert!(out["normalization_error"].is_string());
        assert!(!out.to_string().contains("not-json-secret"));
    }

    #[test]
    fn strict_identity_ignores_examples_and_models_but_preserves_semantics() {
        let a = noul();
        let base = normalized(&a);
        let mut b = a.clone();
        let mut request: Value = serde_json::from_slice(&b.request).unwrap();
        request["state"] = json!("different example");
        request["model"] = json!("different-model");
        request["questions"]["neighbor"] = json!({"type":"noul","instructions":"Other?"});
        b.request = serde_json::to_vec(&request).unwrap();
        assert_eq!(
            base["answers"][0]["group_id"],
            normalized(&b)["answers"][0]["group_id"]
        );
        b.source = "another-source".into();
        assert_ne!(
            base["answers"][0]["group_id"],
            normalized(&b)["answers"][0]["group_id"]
        );
        b.source = a.source.clone();
        b.task_version = Some("rule-v2".into());
        assert_ne!(
            base["answers"][0]["group_id"],
            normalized(&b)["answers"][0]["group_id"]
        );
        b.task_version = None;
        request["questions"]["urgent"]["instructions"] = json!("Urgent by age 18?");
        b.request = serde_json::to_vec(&request).unwrap();
        assert_ne!(
            base["answers"][0]["definition_id"],
            normalized(&b)["answers"][0]["definition_id"]
        );
    }

    #[test]
    fn canonical_definition_and_order_sensitive_presentation_are_distinct() {
        let mut a = noul();
        let mut b = a.clone();
        a.request=br#"{"questions":{"q":{"type":"choice","instructions":"Pick","criteria":{"a":"A","b":"B"}}}}"#.to_vec();
        b.request=br#"{"questions":{"q":{"criteria":{"b":"B","a":"A"},"instructions":"Pick","type":"choice"}}}"#.to_vec();
        let a = normalized(&a);
        let b = normalized(&b);
        assert_eq!(
            a["answers"][0]["definition_id"],
            b["answers"][0]["definition_id"]
        );
        assert_ne!(
            a["answers"][0]["presentation_id"],
            b["answers"][0]["presentation_id"]
        );
        assert_ne!(a["answers"][0]["group_id"], b["answers"][0]["group_id"]);
        assert_eq!(
            a["answers"][0]["candidate_id"],
            b["answers"][0]["candidate_id"]
        );
    }

    #[test]
    fn changed_candidate_descriptions_and_score_order_change_identity() {
        let mut a = noul();
        a.request=serde_json::to_vec(&json!({"questions":{"q":{"type":"choice","instructions":"Pick","criteria":{"a0":"First person","a1":"Second person"}}}})).unwrap();
        let mut b = a.clone();
        let mut req: Value = serde_json::from_slice(&b.request).unwrap();
        req["questions"]["q"]["criteria"]["a0"] = json!("Different person");
        b.request = serde_json::to_vec(&req).unwrap();
        assert_ne!(
            normalized(&a)["answers"][0]["candidate_id"],
            normalized(&b)["answers"][0]["candidate_id"]
        );
        assert_ne!(
            fingerprint("def", &json!({"criteria":["Low","High"]})),
            fingerprint("def", &json!({"criteria":["High","Low"]}))
        );
    }

    #[test]
    fn credentials_and_custom_keys_are_removed_everywhere_retained() {
        let secret = "forwarded-credential-abc";
        let mut c = capture(
            json!({"state":{"password":"nested-password","message":secret},"model":secret,"unknown":{"Authorization":format!("Bearer {secret}"),"echo":secret},"questions":{"q":{"type":"noul","instructions":format!("contains {secret}"),"email":"person@example.test","extra":"nested-password"}}}),
            json!({"answers":{"q":{"type":"noul","noul":0.9,"unknown":{"secret":secret,"echo":secret}}},"other":secret,"headers":{"x-private":secret}}),
        );
        c.id = secret.into();
        c.source = secret.into();
        c.task_version = Some(secret.into());
        c.transport_error = Some(secret.into());
        c.secret = Some(secret.into());
        let options = NormalizeOptions {
            capture_state: true,
            redact_keys: vec!["email".into()],
            ..Default::default()
        };
        let out = normalize(&c, &options);
        let text = out.to_string();
        for forbidden in [
            secret,
            "nested-password",
            "person@example.test",
            "x-private",
        ] {
            assert!(!text.contains(forbidden), "{forbidden} leaked: {text}");
        }
        assert_eq!(out["state_retained"], true);
        assert!(out["answers"][0]["definition"]["extra"].is_string());
        assert_eq!(out["response_extra"]["other"], REDACTED);
    }

    #[test]
    fn credential_redaction_deduplicates_and_preserves_overlap_priority() {
        let mut secrets = vec!["bcd".to_owned(), "abc".to_owned()];
        for _ in 0..2048 {
            secrets.extend(["bcd".to_owned(), "abc".to_owned()]);
        }
        secrets.extend(["long-secret".to_owned(), "long-secret".to_owned()]);
        let unique = ordered_unique_secrets(&secrets);
        assert_eq!(unique.len(), 3);
        assert_eq!(unique, ["long-secret", "bcd", "abc"]);
        assert_eq!(
            scrub_test_string("abcd long-secret", &unique),
            format!("a{REDACTED} {REDACTED}")
        );
        let reverse_secrets = ["abc".into(), "bcd".into()];
        let reverse = ordered_unique_secrets(&reverse_secrets);
        assert_eq!(scrub_test_string("abcd", &reverse), format!("{REDACTED}d"));
        let longest_secrets = ["abc".into(), "abcd".into()];
        let longest = ordered_unique_secrets(&longest_secrets);
        assert_eq!(scrub_test_string("abcd", &longest), REDACTED);
    }

    #[test]
    fn credential_redaction_bounds_markers_and_preserves_utf8() {
        for (value, secrets, expected) in [
            ("EDE", vec!["E", "D", "E", "D"], REDACTED.repeat(3)),
            ("é秘密é", vec!["秘密", "é"], REDACTED.repeat(3)),
            ("abcabc", vec!["abc"], REDACTED.repeat(2)),
            ("", vec!["x"], String::new()),
            ("unchanged", vec!["missing", ""], "unchanged".into()),
        ] {
            let output = scrub_test_string(value, &secrets);
            assert_eq!(output.len(), expected.len());
            assert_eq!(output, expected);
            assert!(output.len() <= value.len() * REDACTED.len());
        }
    }

    #[test]
    fn normalization_never_rescans_generated_credential_markers() {
        let mut c = capture(
            json!({
                "password": ["E", "D", "E", "D", "E", "D"],
                "message": "E",
                "questions": {"q": {"type": "noul", "instructions": "E"}}
            }),
            json!({"answers": {"q": {"type": "noul", "noul": 0.7}}, "E": "E"}),
        );
        c.secret = Some("E".into());
        c.local_token = Some("D".into());
        let output = normalized(&c);
        assert!(output.to_string().len() < 4096);
        assert_eq!(output["request_extra"]["message"], REDACTED);
        assert_eq!(output["response_extra"][REDACTED], REDACTED);
        assert_eq!(output["answers"][0]["definition"]["instructions"], REDACTED);
        assert_eq!(output["answers"][0]["definition_redacted"], true);
    }

    #[test]
    fn structured_sensitive_values_cannot_leak_through_provider_echoes() {
        let options = NormalizeOptions {
            capture_state: true,
            redact_keys: vec!["private_contact".into()],
            ..Default::default()
        };
        let c = capture(
            json!({
                "headers": {"Authorization": ["Bearer   array-header-credential"]},
                "state": {"private_contact": {"addresses": ["private@example.test"]}, "client_token": "local-client-token-secret"},
                "questions": {"q": {"type": "noul", "instructions": "Flag this?"}}
            }),
            json!({
                "answers": {"q": {"type": "noul", "noul": 0.7}},
                "debug": "array-header-credential private@example.test local-client-token-secret"
            }),
        );
        let output = normalize(&c, &options);
        let retained = output.to_string();
        assert!(!retained.contains("array-header-credential"), "{retained}");
        assert!(!retained.contains("private@example.test"), "{retained}");
        assert!(
            !retained.contains("local-client-token-secret"),
            "{retained}"
        );
        assert_eq!(
            output["response_extra"]["debug"],
            format!("{REDACTED} {REDACTED} {REDACTED}")
        );

        let mut exported = normalized(&noul());
        exported["request_extra"] = json!({"private_key": ["imported-array-credential"]});
        exported["response_extra"] = json!({"echo": "imported-array-credential"});
        let imported = import_records(&exported.to_string(), "observer-jsonl", &options).unwrap();
        assert!(
            !imported[0]
                .to_string()
                .contains("imported-array-credential")
        );
    }

    #[test]
    fn imported_workspace_access_headers_redact_echoes_before_grouping() {
        let token = format!("jo_access_{}", "a".repeat(64));
        let echo_key = format!("echo-{token}");
        let definition = json!({
            "type": "noul",
            "instructions": format!("Does {token} apply?")
        });
        let response = json!({"type": "noul", "noul": 0.7});
        let options = NormalizeOptions::default();
        for format in ["observer-jsonl", "systemone-capture"] {
            for (header, value) in [
                ("X-Observer-Access", json!(token)),
                ("x-observer-access", json!([token])),
            ] {
                let mut record = if format == "observer-jsonl" {
                    let mut record = normalized(&noul());
                    record["answers"][0]["definition"] = definition.clone();
                    record["request_extra"] = json!({"echo": token});
                    record["response_extra"] = json!({echo_key.clone(): token});
                    record
                } else {
                    json!({
                        "id": "first",
                        "source": "imported-sdk",
                        "provider": "typesafe",
                        "timestamp": 1_000,
                        "status": 200,
                        "capture_complete": true,
                        "duration_ms": 10.0,
                        "request": {"questions": {"q": definition}, "echo": token},
                        "response": {"answers": {"q": response}, echo_key.clone(): token}
                    })
                };
                record["request_headers"] = json!({header: value});
                let mut groups = Vec::new();
                for id in ["first", "second"] {
                    record["id"] = json!(id);
                    let imported = import_records(&record.to_string(), format, &options).unwrap();
                    let retained = &imported[0];
                    assert!(
                        !retained.to_string().contains(&token),
                        "{format}: {retained}"
                    );
                    assert_eq!(retained["request_extra"]["echo"], REDACTED);
                    assert_eq!(
                        retained["response_extra"][format!("echo-{REDACTED}")],
                        REDACTED
                    );
                    let answer = &retained["answers"][0];
                    assert_eq!(
                        answer["definition"]["instructions"],
                        format!("Does {REDACTED} apply?")
                    );
                    assert_eq!(answer["definition_redacted"], true);
                    assert!(answer["family_id"].is_null());
                    groups.push(answer["group_id"].clone());
                }
                assert_ne!(
                    groups[0], groups[1],
                    "{format}: redacted definitions pooled"
                );
            }
        }
    }

    #[test]
    fn untimed_observer_imports_get_a_valid_import_time_without_fabricated_event_time() {
        let mut record = normalized(&noul());
        record["timestamp"] = Value::Null;
        let before = chrono::Utc::now().timestamp_millis();
        let imported = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        let after = chrono::Utc::now().timestamp_millis();
        assert!(imported[0]["timestamp"].is_null());
        let imported_at = imported[0]["imported_at"]
            .as_i64()
            .expect("Storage needs an import timestamp");
        assert!((before..=after).contains(&imported_at));
        assert_eq!(imported[0]["timestamp_basis"], "import");
        let again = import_records(
            &imported[0].to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(again[0]["imported_at"], imported_at);

        record["imported_at"] = Value::Null;
        let both_null = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert!(both_null[0]["timestamp"].is_null());
        assert!(both_null[0]["imported_at"].as_i64().unwrap() >= before);
        assert_eq!(both_null[0]["timestamp_basis"], "import");

        record["imported_at"] = json!(before - 1_000);
        record["timestamp_basis"] = json!("provider");
        let supplied_time = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(supplied_time[0]["imported_at"], before - 1_000);
        assert_eq!(supplied_time[0]["timestamp_basis"], "import");

        record["timestamp"] = json!(before - 2_000);
        record["imported_at"] = Value::Null;
        let timed = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(timed[0]["timestamp"], before - 2_000);
        assert!(timed[0]["imported_at"].is_null());
        assert_eq!(timed[0]["timestamp_basis"], "provider");

        for invalid in [json!("yesterday"), json!(1.5), json!(u64::MAX)] {
            record["imported_at"] = invalid;
            assert!(
                import_records(
                    &record.to_string(),
                    "observer-jsonl",
                    &NormalizeOptions::default()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn usage_counts_must_fit_storage_before_they_can_establish_known_cost() {
        let options = NormalizeOptions {
            input_price_per_million: Some(1.0),
            output_price_per_million: Some(1.0),
            ..Default::default()
        };
        for field in ["input_tokens", "output_tokens"] {
            let mut c = noul();
            let mut response: Value = serde_json::from_slice(&c.response).unwrap();
            response["usage"] = json!({"input_tokens": 10, "output_tokens": 3});
            response["usage"][field] = json!(i64::MAX as u64 + 1);
            c.response = serde_json::to_vec(&response).unwrap();
            let normalized = normalize(&c, &options);
            assert!(normalized[field].is_null());
            assert!(normalized["cost_usd"].is_null());
            assert!(normalized["cost_basis"].is_null());

            let mut record = normalized.clone();
            record[field] = json!(i64::MAX as u64 + 1);
            assert!(import_records(&record.to_string(), "observer-jsonl", &options).is_err());
            record[field] = json!(i64::MAX);
            let imported = import_records(&record.to_string(), "observer-jsonl", &options).unwrap();
            assert_eq!(imported[0][field], i64::MAX);
        }
    }

    #[test]
    fn raw_state_requires_opt_in_and_unknown_fields_survive_redacted() {
        let mut c = noul();
        let mut req: Value = serde_json::from_slice(&c.request).unwrap();
        req["new_option"] = json!({"n":7});
        c.request = serde_json::to_vec(&req).unwrap();
        let out = normalized(&c);
        assert_eq!(out["state_retained"], false);
        assert!(out["state"].is_null());
        assert_eq!(out["request_extra"]["new_option"]["n"], 7);
        assert_eq!(
            normalize(
                &c,
                &NormalizeOptions {
                    capture_state: true,
                    ..Default::default()
                }
            )["state"],
            "example"
        );
    }

    #[test]
    fn structured_definition_state_is_retained_as_semantics_not_input() {
        let mut c = noul();
        let mut req: Value = serde_json::from_slice(&c.request).unwrap();
        req["state"] = json!({"private_input":"do not persist this snapshot"});
        req["questions"]["urgent"]["instructions"] =
            json!({"state":"Evaluate California residency", "question":"Eligible?"});
        c.request = serde_json::to_vec(&req).unwrap();
        let first = normalized(&c);
        assert!(first["state"].is_null());
        assert_eq!(
            first["answers"][0]["definition"]["instructions"]["state"],
            "Evaluate California residency"
        );
        assert_eq!(first["answers"][0]["definition_redacted"], false);
        req["questions"]["urgent"]["instructions"]["state"] = json!("Evaluate Oregon residency");
        c.request = serde_json::to_vec(&req).unwrap();
        assert_ne!(
            first["answers"][0]["group_id"],
            normalized(&c)["answers"][0]["group_id"]
        );
    }

    #[test]
    fn redacted_definitions_are_isolated_and_never_family_pooled() {
        let options = NormalizeOptions {
            redact_keys: vec!["private_rule".into()],
            ..Default::default()
        };
        let mut c = capture(
            json!({"questions":{"c0":{"type":"noul","instructions":"Look only at the chunk with id \"c0\". Does that code match this description: retries","private_rule":"limit 3"}}}),
            json!({"answers":{"c0":{"type":"noul","noul":0.8}}}),
        );
        c.adapter = Some("jgrep-v1".into());
        let first = normalize(&c, &options);
        c.id = "second-call".into();
        let second = normalize(&c, &options);
        assert_eq!(first["answers"][0]["definition_redacted"], true);
        assert!(first["answers"][0]["family_id"].is_null());
        assert_ne!(
            first["answers"][0]["group_id"],
            second["answers"][0]["group_id"]
        );
        let imported = import_records(&first.to_string(), "observer-jsonl", &options).unwrap();
        assert_eq!(first["answers"], imported[0]["answers"]);
        // Header removal has no marker left, but must remain isolated on import.
        let mut req: Value = serde_json::from_slice(&c.request).unwrap();
        req["questions"]["c0"]
            .as_object_mut()
            .unwrap()
            .remove("private_rule");
        req["questions"]["c0"]["headers"] = json!({"x-rule":"hidden rule"});
        c.request = serde_json::to_vec(&req).unwrap();
        let header_redacted = normalized(&c);
        assert_eq!(header_redacted["answers"][0]["definition_redacted"], true);
        let imported = import_records(
            &header_redacted.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(header_redacted["answers"], imported[0]["answers"]);
    }

    #[test]
    fn imported_removed_definition_fields_cannot_hide_distinct_question_rules() {
        let mut c = capture(
            json!({"questions":{"c0":{"type":"noul","instructions":"Look only at the chunk with id \"c0\". Does that code match this description: retries"}}}),
            json!({"answers":{"c0":{"type":"noul","noul":0.8}}}),
        );
        c.adapter = Some("jgrep-v1".into());
        let mut first = normalized(&c);
        assert!(first["answers"][0]["family_id"].is_string());
        let mut second = first.clone();
        second["id"] = json!("separate-imported-observation");
        for (record, rule, credential) in [
            (
                &mut first,
                "retry at most three times",
                "first-imported-provider-credential",
            ),
            (
                &mut second,
                "retry at most five times",
                "second-imported-provider-credential",
            ),
        ] {
            record["answers"][0]["definition"]["headers"] = json!({
                "x-rule":rule, "Authorization":format!("Bearer {credential}")
            });
            // Imported claims cannot override evidence that normalization
            // removes part of the original definition.
            record["answers"][0]["definition_redacted"] = json!(false);
        }
        let imported = import_records(
            &format!("{first}\n{second}"),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        for record in &imported {
            let answer = &record["answers"][0];
            assert_eq!(answer["definition_redacted"], true);
            assert!(answer["family_id"].is_null());
            assert!(answer["definition"].get("headers").is_none());
            assert!(!record.to_string().contains("imported-provider-credential"));
            let again = import_records(
                &record.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default(),
            )
            .unwrap();
            assert_eq!(again[0]["answers"], record["answers"]);
        }
        assert_ne!(
            imported[0]["answers"][0]["group_id"],
            imported[1]["answers"][0]["group_id"]
        );
    }

    #[test]
    #[ignore = "import scaling measurement; run explicitly with --ignored --nocapture"]
    fn observer_import_large_answer_sets_benchmark() {
        for count in [500, 2_000, 5_000] {
            let questions: Map<String, Value> = (0..count)
                .map(|i| {
                    (
                        format!("question-{i}"),
                        json!({"type":"noul","instructions":"Check this?"}),
                    )
                })
                .collect();
            let answers: Map<String, Value> = questions
                .keys()
                .map(|key| (key.clone(), json!({"type":"noul","noul":0.7})))
                .collect();
            let capture = Capture {
                id: "benchmark".into(),
                source: "benchmark".into(),
                timestamp: 1_800_000_000_000,
                status: 200,
                capture_complete: true,
                request: serde_json::to_vec(&json!({"questions":questions})).unwrap(),
                response: serde_json::to_vec(&json!({"answers":answers})).unwrap(),
                ..Default::default()
            };
            let options = NormalizeOptions::default();
            let text = normalize(&capture, &options).to_string();
            let mut measurements = Vec::new();
            for _ in 0..3 {
                let started = std::time::Instant::now();
                let result = import_records(&text, "observer-jsonl", &options).unwrap();
                assert_eq!(result[0]["answers"].as_array().unwrap().len(), count);
                measurements.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            eprintln!(
                "IMPORT_BENCH {}",
                json!({"answers":count,"bytes":text.len(),"ms":measurements})
            );
        }
    }

    #[test]
    fn observer_import_matches_privacy_evidence_by_key_when_answers_reorder() {
        let mut record = normalized(&capture(
            json!({"questions":{
                "":{"type":"noul","instructions":"Empty key?"},
                "missing-definition":{"type":"noul","instructions":"Unavailable?"},
                "한글":{"type":"noul","instructions":"Private rule?"},
                "ordinary":{"type":"noul","instructions":"Visible rule?"}
            }}),
            json!({"answers":{
                "":{"type":"noul","noul":0.1},
                "missing-definition":{"type":"noul","noul":0.2},
                "한글":{"type":"noul","noul":0.3},
                "ordinary":{"type":"noul","noul":0.4}
            }}),
        ));
        let answers = record["answers"].as_array_mut().unwrap();
        answers[0]["definition_redacted"] = json!(true);
        answers[1]["definition"] = Value::Null;
        answers[2]["definition_redacted"] = json!(true);
        answers.rotate_left(1);
        let imported = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap()
        .remove(0);
        let imported_answers = imported["answers"].as_array().unwrap();
        assert_eq!(imported_answers.len(), 4);
        // A definition-less answer is rebuilt after the question definitions,
        // so input/output positions are deliberately different.
        assert_eq!(record["answers"][0]["key"], "missing-definition");
        assert_eq!(imported_answers[3]["key"], "missing-definition");
        for answer in imported_answers {
            let key = answer["key"].as_str().unwrap();
            assert_eq!(answer["definition_redacted"], matches!(key, "" | "한글"));
            assert_eq!(answer["valid"], key != "missing-definition");
            let original = record["answers"]
                .as_array()
                .unwrap()
                .iter()
                .find(|original| original["key"] == key)
                .unwrap();
            assert_eq!(answer["raw_answer"], original["raw_answer"]);
            if matches!(key, "" | "한글") {
                assert_ne!(answer["group_id"], original["group_id"]);
                assert!(answer["family_id"].is_null());
            }
        }
        let duplicate = record["answers"][0].clone();
        record["answers"].as_array_mut().unwrap().push(duplicate);
        let error = import_records(
            &record.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("Duplicate answer key"));
    }

    #[test]
    fn literal_redaction_marker_cannot_enable_recurring_pooling() {
        let mut c = noul();
        let mut req: Value = serde_json::from_slice(&c.request).unwrap();
        req["questions"]["urgent"]["instructions"] = json!("Check [REDACTED] rule");
        c.request = serde_json::to_vec(&req).unwrap();
        let a = normalized(&c);
        c.id = "another-call".into();
        assert_ne!(
            a["answers"][0]["group_id"],
            normalized(&c)["answers"][0]["group_id"]
        );
    }

    #[test]
    fn incomplete_usage_and_prices_never_produce_a_cost() {
        let options = NormalizeOptions {
            input_price_per_million: Some(1.0),
            output_price_per_million: Some(0.0),
            ..Default::default()
        };
        let mut c = noul();
        for usage in [
            json!({"input_tokens":-1,"output_tokens":0}),
            json!({"input_tokens":1.5,"output_tokens":0}),
            json!({"input_tokens":100}),
            json!({}),
        ] {
            c.response = serde_json::to_vec(
                &json!({"usage":usage,"answers":{"urgent":{"type":"noul","noul":0.8}}}),
            )
            .unwrap();
            assert!(normalize(&c, &options)["cost_usd"].is_null());
        }
        let c = noul();
        assert!(
            normalize(
                &c,
                &NormalizeOptions {
                    input_price_per_million: Some(1.0),
                    ..Default::default()
                }
            )["cost_usd"]
                .is_null()
        );
    }

    #[test]
    fn indexed_family_is_explicit_exact_and_preserves_changed_numbers() {
        let mut c = capture(
            json!({"questions":{
                "c0":{"type":"noul","instructions":"Look only at the chunk with id \"c0\". Does that code match this description: age over 18"},
                "c1":{"type":"noul","instructions":"Look only at the chunk with id \"c1\". Does that code match this description: age over 18"},
                "c2":{"type":"noul","instructions":"Look only at the chunk with id \"c2\". Does that code match this description: age over 21"}
            }}),
            json!({"answers":{}}),
        );
        assert!(normalized(&c)["answers"][0]["family_id"].is_null());
        c.adapter = Some("jgrep-v1".into());
        let out = normalized(&c);
        assert_eq!(
            out["answers"][0]["family_id"],
            out["answers"][1]["family_id"]
        );
        assert_ne!(out["answers"][0]["group_id"], out["answers"][1]["group_id"]);
        assert_ne!(
            out["answers"][0]["family_id"],
            out["answers"][2]["family_id"]
        );
        let mut req: Value = serde_json::from_slice(&c.request).unwrap();
        req["questions"]["c0"]["instructions"] = json!(
            "Look only at the chunk with id \"c1\". Does that code match this description: age over 18"
        );
        c.request = serde_json::to_vec(&req).unwrap();
        assert!(normalized(&c)["answers"][0]["family_id"].is_null());
    }

    #[test]
    fn observer_roundtrip_recomputes_identity_and_keeps_event_provenance() {
        let original = normalized(&noul());
        let mut exported = original.clone();
        exported["answers"][0]["group_id"] = json!("forged");
        exported["answers"][0]["valid"] = json!(false);
        exported["answers"][0]["value"] = json!(0.0);
        exported["labels"] =
            json!([{"key":"urgent","label":"correct","source":"manual","reviewer":"local"}]);
        let imported = import_records(
            &exported.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(imported[0]["answers"], original["answers"]);
        assert_eq!(imported[0]["source_event_id"], original["id"]);
        assert_eq!(imported[0]["labels"][0]["source"], "manual");
        let again = import_records(
            &imported[0].to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(again[0]["source_event_id"], imported[0]["source_event_id"]);
        assert_eq!(again[0]["answers"], imported[0]["answers"]);
    }

    #[test]
    fn observer_roundtrip_preserves_malformed_definitions_and_revalidates_answers() {
        let options = NormalizeOptions::default();
        let mut groups = BTreeSet::new();
        for definition in [
            json!(42),
            json!(-1.5),
            json!(true),
            json!(""),
            json!("한글"),
            json!([]),
            json!([null, 1, {"instructions": "nested"}]),
            Value::Null,
        ] {
            for raw in [json!({"type": "noul", "noul": 0.8}), Value::Null] {
                let original = normalized(&capture(
                    json!({"questions": {"q": definition}}),
                    json!({"answers": {"q": raw}}),
                ));
                let answer = &original["answers"][0];
                assert_eq!(answer["definition"], definition);
                assert_eq!(answer["raw_answer"], raw);
                assert_eq!(answer["valid"], false);
                if !raw.is_null() {
                    assert!(groups.insert(answer["group_id"].as_str().unwrap().to_owned()));
                }
                let mut exported = original.clone();
                for field in ["group_id", "definition_id", "presentation_id", "family_id"] {
                    exported["answers"][0][field] = json!("forged");
                }
                exported["answers"][0]["valid"] = json!(true);
                exported["answers"][0]["value"] = json!(123);
                let imported = import_records(&exported.to_string(), "observer-jsonl", &options)
                    .unwrap_or_else(|error| panic!("{definition}: {error:#}"))
                    .remove(0);
                assert_eq!(imported["answers"], original["answers"]);
                let again = import_records(&imported.to_string(), "observer-jsonl", &options)
                    .unwrap()
                    .remove(0);
                assert_eq!(again["answers"], original["answers"]);
                assert_eq!(again["source_event_id"], original["id"]);
            }
        }
        // Null is retained evidence; an absent required field is still invalid.
        for field in ["definition", "raw_answer"] {
            let mut missing = normalized(&noul());
            missing["answers"][0].as_object_mut().unwrap().remove(field);
            assert!(import_records(&missing.to_string(), "observer-jsonl", &options).is_err());
        }
    }

    #[test]
    fn observer_roundtrip_keeps_redacted_scalar_and_array_definitions_isolated() {
        let secret = "malformed-definition-test-secret";
        for (key, definition) in [
            ("q", json!([secret, {"api_key": secret}])),
            (
                "authorization",
                json!({"type": "noul", "instructions": secret}),
            ),
        ] {
            let mut groups = Vec::new();
            for id in ["first", "second"] {
                let mut c = capture(
                    json!({"questions": {key: definition}}),
                    json!({"answers": {key: {"type": "noul", "noul": 0.8}}}),
                );
                c.id = id.into();
                c.secret = Some(secret.into());
                let original = normalized(&c);
                assert!(!original.to_string().contains(secret));
                let answer = &original["answers"][0];
                assert!(!answer["definition"].is_object());
                assert_eq!(answer["valid"], false);
                assert_eq!(answer["definition_redacted"], true);
                assert!(answer["family_id"].is_null());
                let imported = import_records(
                    &original.to_string(),
                    "observer-jsonl",
                    &NormalizeOptions::default(),
                )
                .unwrap()
                .remove(0);
                assert!(!imported.to_string().contains(secret));
                assert_eq!(imported["answers"], original["answers"]);
                groups.push(imported["answers"][0]["group_id"].clone());
            }
            assert_ne!(groups[0], groups[1]);
        }
    }

    #[test]
    fn observer_roundtrip_preserves_empty_question_and_label_keys() {
        let mut original = normalized(&capture(
            json!({"questions":{"":{"type":"noul","instructions":"Urgent?"}}}),
            json!({"answers":{"":{"type":"noul","noul":0.8}}}),
        ));
        original["labels"] = json!([{"key":"","label":"correct","source":"manual"}]);
        assert_eq!(original["answers"][0]["valid"], true);
        let imported = import_records(
            &original.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(imported[0]["answers"], original["answers"]);
        assert_eq!(imported[0]["labels"][0]["key"], "");
        assert_eq!(imported[0]["labels"][0]["label"], "correct");
        for invalid in [Value::Null, json!(42), json!({})] {
            let mut record = original.clone();
            record["answers"][0]["key"] = invalid.clone();
            assert!(
                import_records(
                    &record.to_string(),
                    "observer-jsonl",
                    &NormalizeOptions::default()
                )
                .is_err()
            );
            let mut record = original.clone();
            record["labels"][0]["key"] = invalid;
            assert!(
                import_records(
                    &record.to_string(),
                    "observer-jsonl",
                    &NormalizeOptions::default()
                )
                .is_err()
            );
        }
    }

    fn receipt() -> Value {
        serde_json::from_str(include_str!("../fixtures/model/jevrouter-receipt.json")).unwrap()
    }

    #[test]
    fn receipt_keeps_policy_separate_without_invented_definitions_or_timings() {
        let imported = import_records(
            &serde_json::to_string_pretty(&receipt()).unwrap(),
            "jevrouter-receipt",
            &NormalizeOptions::default(),
        )
        .unwrap();
        let r = &imported[0];
        for key in ["timestamp", "status", "duration_ms", "cost_usd"] {
            assert!(r[key].is_null(), "{key}");
        }
        assert_eq!(r["actions"][0]["decision"]["selected"], "safe");
        assert_eq!(r["event_kind"], "application_action");
        assert!(r["answers"].as_array().unwrap().is_empty());
        assert_eq!(
            r["actions"][0]["raw_jev"]["answers"]["tool"]["choice"],
            "unsafe"
        );
        assert!(r["labels"].as_array().unwrap().is_empty());
        assert!(!r.to_string().contains("input secret"));
        let roundtrip = import_records(
            &r.to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert_eq!(roundtrip[0]["answers"], r["answers"]);
        let mut no_model = receipt();
        no_model["raw_jev"] = Value::Null;
        assert!(
            import_records(
                &no_model.to_string(),
                "jevrouter-receipt",
                &NormalizeOptions::default()
            )
            .unwrap()[0]["answers"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn imported_receipt_inputs_obey_new_capture_preference() {
        let retained = import_records(
            &receipt().to_string(),
            "jevrouter-receipt",
            &NormalizeOptions {
                capture_state: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(retained[0].to_string().contains("input secret"));
        let scrubbed = import_records(
            &retained[0].to_string(),
            "observer-jsonl",
            &NormalizeOptions::default(),
        )
        .unwrap();
        assert!(!scrubbed[0].to_string().contains("input secret"));
    }

    #[test]
    fn import_is_atomic_strict_and_does_not_deduplicate_equal_independent_calls() {
        let mut a = normalized(&noul());
        let mut b = a.clone();
        b["id"] = json!("independent-2");
        assert_eq!(
            import_records(
                &format!("{a}\n{b}"),
                "observer-jsonl",
                &NormalizeOptions::default()
            )
            .unwrap()
            .len(),
            2
        );
        assert!(
            import_records(
                &format!("{a}\n{{broken"),
                "observer-jsonl",
                &NormalizeOptions::default()
            )
            .is_err()
        );
        a["duration_ms"] = json!(-1);
        assert!(
            import_records(
                &a.to_string(),
                "observer-jsonl",
                &NormalizeOptions::default()
            )
            .is_err()
        );
        assert!(import_records("{}", "made-up-format", &NormalizeOptions::default()).is_err());
        for invalid in ["null", "false", "1", "[]", r#""not a record""#] {
            assert!(
                import_records(invalid, "observer-jsonl", &NormalizeOptions::default()).is_err()
            );
        }
    }

    #[test]
    fn samples_are_synthetic_with_versions_errors_families_and_labels() {
        let samples = sample_records();
        assert_eq!(samples.len(), 720);
        assert!(
            samples.iter().all(|r| r["sample"] == true
                && (r["cost_usd"].is_null() || r["cost_basis"] == "synthetic"))
        );
        assert!(samples.iter().any(|r| r["status"] == 529));
        assert!(
            samples
                .iter()
                .any(|r| !r["labels"].as_array().unwrap().is_empty())
        );
        assert!(
            samples
                .iter()
                .flat_map(|r| r["answers"].as_array().unwrap())
                .any(|a| a["family_id"].is_string())
        );
        assert!(
            samples
                .iter()
                .filter(|r| r["status"] == 200)
                .flat_map(|r| r["answers"].as_array().unwrap())
                .all(|a| a["valid"] == true)
        );
        let versions: BTreeSet<_> = samples
            .iter()
            .flat_map(|r| r["answers"].as_array().unwrap())
            .filter(|a| a["key"] == "support_routing")
            .map(|a| a["definition_id"].as_str().unwrap())
            .collect();
        assert_eq!(versions.len(), 2);
    }
}

#[cfg(test)]
#[path = "model_compatibility_tests.rs"]
mod compatibility_tests;

#[cfg(test)]
#[path = "import_budget_tests.rs"]
mod import_budget_tests;

#[cfg(test)]
#[path = "receipt_privacy_tests.rs"]
mod receipt_privacy_tests;

#[cfg(test)]
#[path = "redaction_tests.rs"]
mod redaction_tests;

#[cfg(test)]
#[path = "envelope_privacy_tests.rs"]
mod envelope_privacy_tests;
