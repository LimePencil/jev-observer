//! Bounded credential discovery, redaction, and input retention policy.

use super::budget::{ImportLimitError, limited_add};
use super::types::NormalizeOptions;
use super::validation::accepts_text;
use crate::catalog;
use crate::catalog::Profile;
use aho_corasick::AhoCorasick;
use aho_corasick::AhoCorasickKind;
use aho_corasick::MatchKind;
use anyhow::Result;
use anyhow::ensure;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

pub(super) const REDACTED: &str = "[REDACTED]";
pub(super) const INPUT_FIELDS: [&str; 9] = [
    "state", "images", "image", "audio", "video", "assets", "media", "videos", "input",
];
#[derive(Clone, Copy)]
pub(super) struct RedactionLimits {
    pub(super) patterns: usize,
    pub(super) pattern_bytes: usize,
    pub(super) work: usize,
}

pub(super) const REDACTION_LIMITS: RedactionLimits = RedactionLimits {
    patterns: 32_768,
    pattern_bytes: 1024 * 1024,
    work: 64 * 1024 * 1024,
};

pub(super) const REDACTION_LIMIT_MESSAGE: &str =
    "Credential redaction exceeded its work limit; request and response content were not retained";
pub(super) const REDACTION_COLLISION_MESSAGE: &str = "Credential redaction caused an identifier or object key collision; request and response content were not retained";
pub const IMPORT_REDACTION_COLLISION_MESSAGE: &str = "Import rejected because credential redaction makes distinct identifiers or object keys identical.";

#[derive(Debug)]
pub struct RedactionCollisionError;

impl std::fmt::Display for RedactionCollisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(IMPORT_REDACTION_COLLISION_MESSAGE)
    }
}

impl std::error::Error for RedactionCollisionError {}

pub(super) const REDACTION_VALIDATION_MESSAGE: &str = "Credential redaction changed answer validation information; request and response content were not retained";
pub const IMPORT_REDACTION_VALIDATION_MESSAGE: &str = "Import rejected because credential redaction changes answer validation fields or the provider profile.";

#[derive(Debug)]
pub struct RedactionValidationError;

impl std::fmt::Display for RedactionValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(IMPORT_REDACTION_VALIDATION_MESSAGE)
    }
}

impl std::error::Error for RedactionValidationError {}

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

pub(super) fn collect_secret_string(value: &str, found: &mut Vec<String>) {
    if value.is_empty() || value == REDACTED {
        return;
    }
    found.push(value.to_owned());
    if let Some((scheme, credential)) = value.split_once(char::is_whitespace)
        && (scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("basic"))
    {
        // Match the supplied credential component as well as the full header.
        // Basic stays encoded: do not decode or guess other representations.
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

pub(super) fn collect_secrets(value: &Value, options: &NormalizeOptions, found: &mut Vec<String>) {
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

pub(super) struct SecretMatcher<'a> {
    pub(super) secrets: Vec<&'a str>,
    pub(super) prefilter: Option<AhoCorasick>,
    pub(super) remaining: usize,
}

impl<'a> SecretMatcher<'a> {
    pub(super) fn new(secrets: Vec<&'a str>, limits: RedactionLimits) -> Result<Self> {
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

pub(super) fn scrub_string(value: &str, matcher: &mut SecretMatcher<'_>) -> Result<String> {
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

#[derive(Clone, Copy)]
pub(super) enum IdentifierNamespace<'a> {
    Question,
    Choice(&'a str),
    DecisionName(&'a str),
    ScoreLabel(&'a str, u64),
}

#[derive(Default)]
pub(super) struct SemanticIdentifiers<'a> {
    pub(super) questions: BTreeMap<String, &'a str>,
    pub(super) choices: BTreeMap<&'a str, BTreeMap<String, &'a str>>,
    names: BTreeMap<&'a str, BTreeMap<String, &'a str>>,
    levels: BTreeMap<(&'a str, u64), BTreeMap<String, &'a str>>,
    pub(super) count: usize,
}

impl<'a> SemanticIdentifiers<'a> {
    pub(super) fn observe(
        &mut self,
        namespace: IdentifierNamespace<'a>,
        original: &'a str,
        retained: &str,
        matcher: &mut SecretMatcher<'_>,
    ) -> Result<()> {
        let question_bytes = match namespace {
            IdentifierNamespace::Question => 0,
            IdentifierNamespace::Choice(question)
            | IdentifierNamespace::DecisionName(question)
            | IdentifierNamespace::ScoreLabel(question, _) => question.len(),
        };
        // Bound both B-tree lookups/comparisons and retained-key storage before
        // any owned allocation. Original identifiers borrow the input JSON;
        // they never enter retained output or an error message.
        let comparisons = (self.count.saturating_add(1).ilog2() as usize + 1) * 12;
        let work = question_bytes
            .saturating_add(retained.len())
            .saturating_add(1)
            .saturating_mul(comparisons)
            .saturating_add(original.len())
            .saturating_add(retained.len())
            .saturating_add(128);
        matcher.charge(work)?;
        let identifiers = match namespace {
            IdentifierNamespace::Question => &mut self.questions,
            IdentifierNamespace::Choice(question) => self.choices.entry(question).or_default(),
            IdentifierNamespace::DecisionName(question) => self.names.entry(question).or_default(),
            IdentifierNamespace::ScoreLabel(question, index) => {
                self.levels.entry((question, index)).or_default()
            }
        };
        if let Some(previous) = identifiers.get(retained) {
            ensure!(*previous == original, RedactionCollisionError);
        } else {
            identifiers.insert(retained.to_owned(), original);
            self.count += 1;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Default)]
enum PrivacyContext<'a> {
    #[default]
    Other,
    Request(&'a Profile),
    Response(&'a Profile),
    CloudflareResponse(&'a Profile),
    Questions(&'a Profile),
    Answers(&'a Profile),
    Definition(&'a Profile, Option<&'a str>),
    Answer(&'a Profile, Option<&'a str>),
    NoulCriteria(&'a Profile),
    SemanticMap,
    Observer(&'a Profile),
    ObserverAnswers(&'a Profile),
    ObserverAnswer(&'a Profile, Option<&'a str>),
    ChoiceCriteria(&'a Profile, &'a str),
    ChoiceProbabilities(&'a str),
    Identifier(IdentifierNamespace<'a>),
    TypedOptions(&'a str),
    TypedOption(&'a str),
    DecisionLevels(&'a str),
    DecisionLevel(&'a str, u64),
    ScoreProbabilities(&'a str),
    ScoreProbability(&'a str, u64),
}

impl<'a> PrivacyContext<'a> {
    fn field(self, object: &Value, key: &'a str) -> Self {
        match (self, key) {
            (Self::Request(profile), "questions") => Self::Questions(profile),
            (Self::Response(profile), "answers") => Self::Answers(profile),
            (Self::CloudflareResponse(profile), "result") => Self::Response(profile),
            (Self::Questions(profile), _) => Self::Definition(profile, Some(key)),
            (Self::Answers(profile), _) => Self::Answer(profile, Some(key)),
            (Self::Observer(profile), "answers") => Self::ObserverAnswers(profile),
            (Self::ObserverAnswer(profile, question), "definition") => {
                Self::Definition(profile, question)
            }
            (Self::ObserverAnswer(profile, question), "raw_answer") => {
                Self::Answer(profile, question)
            }
            (Self::ObserverAnswer(_, _), "key") => Self::Identifier(IdentifierNamespace::Question),
            (Self::Definition(profile, Some(question)), "choices") if profile.decisions => {
                Self::TypedOptions(question)
            }
            (Self::Definition(profile, Some(question)), "levels") if profile.decisions => {
                Self::DecisionLevels(question)
            }
            (
                Self::Definition(profile, Some(question)) | Self::Answer(profile, Some(question)),
                "name",
            ) if profile.decisions => Self::Identifier(IdentifierNamespace::DecisionName(question)),
            (Self::Answer(profile, Some(question)), "probabilities")
                if profile.decisions && object["type"] == "score" =>
            {
                Self::ScoreProbabilities(question)
            }
            (
                Self::DecisionLevel(question, index) | Self::ScoreProbability(question, index),
                "label",
            ) => Self::Identifier(IdentifierNamespace::ScoreLabel(question, index)),
            (Self::Answer(profile, Some(question)), "probabilities")
                if profile.decisions && object["type"] == "choice" =>
            {
                Self::TypedOptions(question)
            }
            (Self::TypedOption(question), "value") => {
                Self::Identifier(IdentifierNamespace::Choice(question))
            }
            (Self::Definition(profile, Some(question)), "criteria")
                if object["type"] == "choice" =>
            {
                Self::ChoiceCriteria(profile, question)
            }
            (Self::Answer(_, Some(question)), "choice") if object["type"] == "choice" => {
                Self::Identifier(IdentifierNamespace::Choice(question))
            }
            (Self::Answer(_, Some(question)), "probabilities") if object["type"] == "choice" => {
                Self::ChoiceProbabilities(question)
            }
            (Self::ObserverAnswer(_, _), "probabilities")
                if matches!(
                    object["raw_answer"]["type"].as_str(),
                    Some("choice" | "score")
                ) =>
            {
                Self::SemanticMap
            }
            (Self::Definition(profile, _), "criteria") if object["type"] == "noul" => {
                Self::NoulCriteria(profile)
            }
            (Self::Definition(_, _), "criteria") if object["type"] == "choice" => Self::SemanticMap,
            (Self::Answer(_, _), "probabilities")
                if matches!(object["type"].as_str(), Some("choice" | "score")) =>
            {
                Self::SemanticMap
            }
            (Self::Answer(_, _), "legend") if object["type"] == "score" => Self::SemanticMap,
            _ => Self::Other,
        }
    }

    fn semantic_keys(self) -> bool {
        self.identifier_keys().is_some()
            || matches!(self, Self::NoulCriteria(_) | Self::SemanticMap)
    }

    fn item(self, index: usize, item: &'a Value) -> Self {
        match self {
            Self::ObserverAnswers(profile) => Self::ObserverAnswer(profile, item["key"].as_str()),
            Self::ChoiceCriteria(profile, question) if profile.list_choice => {
                Self::Identifier(IdentifierNamespace::Choice(question))
            }
            Self::TypedOptions(question) => Self::TypedOption(question),
            Self::DecisionLevels(question) => Self::DecisionLevel(question, index as u64),
            Self::ScoreProbabilities(question) => item["value"]
                .as_u64()
                .map_or(Self::Other, |index| Self::ScoreProbability(question, index)),
            _ => Self::Other,
        }
    }

    fn identifier_keys(self) -> Option<IdentifierNamespace<'a>> {
        match self {
            Self::Questions(_) | Self::Answers(_) => Some(IdentifierNamespace::Question),
            Self::ChoiceCriteria(_, question) | Self::ChoiceProbabilities(question) => {
                Some(IdentifierNamespace::Choice(question))
            }
            _ => None,
        }
    }

    fn validates_optional_field(self, object: &Value, key: &str) -> bool {
        match self {
            Self::Definition(profile, _) => {
                (key == "criteria" && matches!(object["type"].as_str(), Some("noul" | "box")))
                    || (key == "instructions" && profile.optional_instructions)
                    || (key == "name" && profile.decisions)
            }
            Self::Answer(profile, _) => {
                (key == "confidence"
                    && profile.optional_confidence
                    && matches!(object["type"].as_str(), Some("choice" | "score")))
                    || (key == "legend" && profile.optional_legend && object["type"] == "score")
                    || (matches!(key, "found" | "present")
                        && profile.box_presence
                        && object["type"] == "box")
                    || (key == "name" && profile.decisions)
                    || (matches!(key, "valid" | "abstain") && profile.reported_abstention)
            }
            Self::TypedOption(_) | Self::DecisionLevel(_, _) => key == "description",
            Self::ObserverAnswer(_, _) => {
                matches!(
                    key,
                    "definition_present"
                        | "raw_answer_present"
                        | "task_version"
                        | "definition_redacted"
                )
            }
            _ => false,
        }
    }

    fn repairs_text_shape(self, key: &str, original: &Value, retained: &Value) -> bool {
        let strict_text = match self {
            Self::Definition(profile, _) => {
                profile.decisions && matches!(key, "instructions" | "name")
            }
            Self::Answer(profile, _) => profile.decisions && key == "name" && !original.is_null(),
            Self::TypedOption(_) => key == "description",
            Self::DecisionLevel(_, _) => matches!(key, "label" | "description"),
            _ => false,
        };
        if strict_text {
            return !original.is_string() && retained.is_string();
        }
        let (profile, optional_null) = match self {
            Self::Definition(profile, _) if key == "instructions" => {
                (profile, profile.optional_instructions)
            }
            Self::NoulCriteria(profile) if matches!(key, "true" | "false") => {
                (profile, profile.nullable_noul_criteria)
            }
            _ => return false,
        };
        !accepts_text(original, profile, optional_null)
            && accepts_text(retained, profile, optional_null)
    }
}

pub(super) fn scrub(
    value: &Value,
    options: &NormalizeOptions,
    matcher: &mut SecretMatcher<'_>,
) -> Result<Value> {
    scrub_with_context(
        value,
        options,
        matcher,
        PrivacyContext::Other,
        &mut SemanticIdentifiers::default(),
    )
}

fn scrub_with_context<'a>(
    value: &'a Value,
    options: &NormalizeOptions,
    matcher: &mut SecretMatcher<'_>,
    context: PrivacyContext<'a>,
    identifiers: &mut SemanticIdentifiers<'a>,
) -> Result<Value> {
    Ok(match value {
        Value::String(original) => {
            let retained = scrub_string(original, matcher)?;
            if let PrivacyContext::Identifier(namespace) = context {
                identifiers.observe(namespace, original, &retained, matcher)?;
            }
            json!(retained)
        }
        Value::Array(values) => Value::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, v)| {
                    scrub_with_context(v, options, matcher, context.item(index, v), identifiers)
                })
                .collect::<Result<_>>()?,
        ),
        Value::Object(values) => {
            let mut out = Map::new();
            for (key, original) in values {
                // Direct semantic-map keys identify questions, options or
                // levels. A label named "headers" is not a header container.
                // Nested extensions still use the ordinary omission rule.
                if !context.semantic_keys()
                    && matches!(
                        key_name(key).as_str(),
                        "headers" | "requestheaders" | "responseheaders"
                    )
                {
                    continue;
                }
                let child = context.field(value, key);
                let retained = if sensitive_key(key, options) {
                    if let PrivacyContext::Identifier(namespace) = child {
                        // Replacing an invalid non-string must not invent a
                        // valid identifier equal to a literal marker elsewhere.
                        let original = original.as_str().ok_or(RedactionValidationError)?;
                        identifiers.observe(namespace, original, REDACTED, matcher)?;
                    }
                    json!(REDACTED)
                } else {
                    scrub_with_context(original, options, matcher, child, identifiers)?
                };
                let retained_key = scrub_string(key, matcher)?;
                // Follow only exact original protocol paths. Optional fields
                // cannot become "not reported" after their names are scrubbed.
                ensure!(
                    retained_key == *key || !context.validates_optional_field(value, key),
                    RedactionValidationError
                );
                // A replacement marker is text, but must not repair a value
                // that the selected profile rejected before redaction.
                ensure!(
                    !context.repairs_text_shape(key, original, &retained),
                    RedactionValidationError
                );
                if let Some(namespace) = context.identifier_keys() {
                    identifiers.observe(namespace, key, &retained_key, matcher)?;
                }
                // Distinct original keys must not silently replace one another,
                // even when one already contains a literal redaction marker.
                ensure!(
                    out.insert(retained_key, retained).is_none(),
                    RedactionCollisionError
                );
            }
            Value::Object(out)
        }
        _ => value.clone(),
    })
}

pub(super) fn ordered_unique_secrets(secrets: &[String]) -> Vec<&str> {
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

fn privacy_secrets(value: &Value, options: &NormalizeOptions, explicit: &[&str]) -> Vec<String> {
    let mut secrets = Vec::new();
    for secret in explicit {
        collect_secret_string(secret, &mut secrets);
    }
    collect_secrets(value, options, &mut secrets);
    secrets
}

fn with_secret_matcher(
    secrets: &[String],
    remaining: &mut usize,
    redact: impl FnOnce(&mut SecretMatcher<'_>) -> Result<Value>,
) -> Result<Value> {
    let mut matcher = SecretMatcher::new(
        ordered_unique_secrets(secrets),
        RedactionLimits {
            work: *remaining,
            ..REDACTION_LIMITS
        },
    )?;
    let result = redact(&mut matcher);
    *remaining = matcher.remaining;
    result
}

pub(super) fn privacy(
    value: &Value,
    options: &NormalizeOptions,
    explicit: &[&str],
    remaining: &mut usize,
) -> Result<Value> {
    let secrets = privacy_secrets(value, options, explicit);
    with_secret_matcher(&secrets, remaining, |matcher| {
        scrub(value, options, matcher)
    })
}

pub(super) fn privacy_with_input_filter(
    value: &mut Value,
    options: &NormalizeOptions,
    explicit: &[&str],
    remaining: &mut usize,
    filter_inputs: impl FnOnce(&mut Value),
) -> Result<Value> {
    // Discover and bound every original secret before discarding inputs. Omitted
    // credentials must still redact retained echoes in explicit-first order.
    let secrets = privacy_secrets(value, options, explicit);
    with_secret_matcher(&secrets, remaining, |matcher| {
        if !options.capture_state {
            filter_inputs(value);
        }
        scrub(value, options, matcher)
    })
}

pub(super) fn privacy_observer(
    value: &mut Value,
    options: &NormalizeOptions,
    remaining: &mut usize,
) -> Result<Value> {
    ensure!(
        value["model_profile"].is_null()
            || value["model_profile"]
                .as_str()
                .is_some_and(crate::catalog::known_profile),
        "Unknown model_profile"
    );
    let mut profile = catalog::profile(
        value["model_profile"]
            .as_str()
            .or(value["provider"].as_str())
            .unwrap_or("typesafe"),
    );
    profile.decisions = value["protocol"] == "openai-decisions";
    let secrets = privacy_secrets(value, options, &[]);
    let retained = with_secret_matcher(&secrets, remaining, |matcher| {
        if !options.capture_state {
            clear_observer_inputs(value);
        }
        scrub_with_context(
            value,
            options,
            matcher,
            PrivacyContext::Observer(&profile),
            &mut SemanticIdentifiers::default(),
        )
    })?;
    // Observer imports select their validator from this retained field. Privacy
    // must not silently substitute a different profile before revalidation.
    ensure!(
        !value["answers"]
            .as_array()
            .is_some_and(|answers| !answers.is_empty())
            || ({
                let mut retained_profile = catalog::profile(
                    retained["model_profile"]
                        .as_str()
                        .or(retained["provider"].as_str())
                        .unwrap_or("typesafe"),
                );
                retained_profile.decisions = retained["protocol"] == "openai-decisions";
                profile == retained_profile
                    && value["protocol"] == retained["protocol"]
                    && value["model_profile"] == retained["model_profile"]
            }),
        RedactionValidationError
    );
    Ok(retained)
}

pub(super) fn privacy_capture_envelope(
    envelope: &mut Value,
    options: &NormalizeOptions,
    explicit: &[&str],
    remaining: &mut usize,
) -> Result<Value> {
    let profile = options.validation_profile();
    let secrets = privacy_secrets(envelope, options, explicit);
    with_secret_matcher(&secrets, remaining, |matcher| {
        if !options.capture_state {
            clear_input_fields(&mut envelope["request"]);
            clear_response_input_fields(&mut envelope["response"]);
            if options.protocol == super::types::Protocol::CloudflareSystemone {
                clear_response_input_fields(&mut envelope["response"]["result"]);
            }
        }
        let mut out = Map::new();
        let mut identifiers = SemanticIdentifiers::default();
        // These root names are generated structure, not captured data. Scrubbing
        // "id" into a different key would make an otherwise valid capture
        // unsaveable. Original body keys and all metadata values still pass
        // through the ordinary privacy rules with one shared secret matcher.
        for (key, value) in envelope.as_object().expect("constructed capture envelope") {
            let value = if sensitive_key(key, options) {
                json!(REDACTED)
            } else {
                let context = match key.as_str() {
                    "request" => PrivacyContext::Request(&profile),
                    "response"
                        if options.protocol == super::types::Protocol::CloudflareSystemone =>
                    {
                        PrivacyContext::CloudflareResponse(&profile)
                    }
                    "response" => PrivacyContext::Response(&profile),
                    _ => PrivacyContext::Other,
                };
                scrub_with_context(value, options, matcher, context, &mut identifiers)?
            };
            out.insert(key.clone(), value);
        }
        Ok(Value::Object(out))
    })
}

pub(super) fn clear_input_fields(value: &mut Value) {
    if let Some(map) = value.as_object_mut() {
        for field in INPUT_FIELDS {
            map.remove(field);
        }
    }
}

fn clear_response_input_fields(response: &mut Value) {
    clear_input_fields(response);
    // Legacy envelope inputs, not arbitrary nested question/result semantics.
    if let Some(metadata) = response.get_mut("original_metadata") {
        clear_input_fields(metadata);
    }
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

pub(super) fn remove_receipt_inputs(receipt: &mut Value) {
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

fn clear_observer_inputs(record: &mut Value) {
    // Preserve required-field validation: do not insert state when absent.
    if let Some(state) = record.get_mut("state") {
        *state = Value::Null;
    }
    if let Some(request) = record.get_mut("request_extra") {
        clear_input_fields(request);
    }
    if let Some(response) = record.get_mut("response_extra") {
        clear_response_input_fields(response);
    }
    if let Some(metadata) = record.get_mut("protocol_metadata") {
        clear_response_input_fields(metadata);
    }
    if let Some(receipt) = record.get_mut("receipt") {
        remove_receipt_inputs(receipt);
    }
    if let Some(actions) = record.get_mut("actions").and_then(Value::as_array_mut) {
        for action in actions {
            remove_receipt_inputs(action);
        }
    }
}
