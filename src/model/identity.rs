//! Stable definition identities and source-reviewed grouping adapters.

use serde_json::Value;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;

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
