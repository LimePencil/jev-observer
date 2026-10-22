//! Stable definition identities and source-reviewed grouping adapters.

use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeSet;

/// Asset references embedded in a question change its meaning independently
/// of its text. Isolate these observations without publishing a content hash.
pub(super) fn semantic_asset_questions(request: &Value) -> BTreeSet<String> {
    let mut patterns: Vec<&str> = request["assets"]
        .as_object()
        .into_iter()
        .flat_map(|assets| assets.keys().map(String::as_str))
        .filter(|key| !key.is_empty())
        .collect();
    if request["media"]
        .as_array()
        .is_some_and(|items| !items.is_empty())
    {
        patterns.extend(["<image:", "<video:"]);
    }
    let Some(questions) = request["questions"].as_object() else {
        return BTreeSet::new();
    };
    if patterns.is_empty() {
        return BTreeSet::new();
    }
    // Bound matcher construction even for malformed provider envelopes.
    if patterns.len() > 1024 || patterns.iter().map(|key| key.len()).sum::<usize>() > 65_536 {
        return questions.keys().cloned().collect();
    }
    let Ok(matcher) = aho_corasick::AhoCorasick::builder()
        .kind(Some(aho_corasick::AhoCorasickKind::NoncontiguousNFA))
        .dense_depth(0)
        .build(patterns)
    else {
        return questions.keys().cloned().collect();
    };
    fn referenced(value: &Value, matcher: &aho_corasick::AhoCorasick) -> bool {
        match value {
            Value::String(text) => matcher.is_match(text),
            Value::Array(values) => values.iter().any(|value| referenced(value, matcher)),
            Value::Object(values) => values.values().any(|value| referenced(value, matcher)),
            _ => false,
        }
    }
    questions
        .iter()
        .filter(|(_, definition)| {
            referenced(&definition["instructions"], &matcher)
                || referenced(&definition["criteria"], &matcher)
        })
        .map(|(key, _)| key.clone())
        .collect()
}

pub(super) fn group_context(
    options: &super::types::NormalizeOptions,
    id: &Value,
    isolated: bool,
) -> Value {
    if options.model_profile.is_none() && options.protocol == super::types::Protocol::Systemone {
        return if isolated { id.clone() } else { Value::Null };
    }
    json!({"protocol": options.protocol, "model_profile": options.profile_name(),
        "observation": if isolated { id.clone() } else { Value::Null }})
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

pub(super) fn fingerprint(prefix: &str, value: &Value) -> String {
    let bytes = serde_json::to_vec(&canonical(value)).expect("JSON values serialize");
    format!("{prefix}_{:x}", Sha256::digest(bytes))
}

pub(super) fn group_fingerprint(
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

pub(super) fn jgrep_family(
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
