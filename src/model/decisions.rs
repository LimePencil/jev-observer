//! OpenAI Decisions uses ordered questions and typed choices. Preserve native
//! definitions/answers; only the in-memory envelope gets ordinal map keys.

use super::validation::{finite, probability};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

#[derive(Debug)]
pub(super) struct ProtocolShapeError;
impl std::fmt::Display for ProtocolShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Decisions requires ordered question and answer arrays")
    }
}
impl std::error::Error for ProtocolShapeError {}

pub(super) fn valid_wire_shape(request: &Value, response: &Value, successful: bool) -> bool {
    request["questions"]
        .as_array()
        .is_some_and(|items| (1..=400).contains(&items.len()))
        && (!successful
            || response["answers"]
                .as_array()
                .is_some_and(|items| items.len() <= 400))
}

pub(super) fn ordinal_maps(request: &mut Value, response: &mut Value) {
    for (body, field) in [(request, "questions"), (response, "answers")] {
        if let Some(values) = body.get_mut(field).filter(|value| value.is_array()) {
            let items = values
                .take()
                .as_array_mut()
                .expect("checked array")
                .drain(..)
                .enumerate()
                .map(|(index, value)| (format!("question:{index}"), value))
                .collect();
            *values = Value::Object(items);
        }
    }
}

pub(crate) fn choice_key(value: &Value) -> Option<String> {
    (value.is_string() || value.is_boolean()).then(|| {
        format!(
            "choice:v1:{}",
            serde_json::to_string(value).expect("JSON value")
        )
    })
}

pub(super) fn probabilities(raw: &Value, kind: &str) -> Value {
    let Some(values) = raw["probabilities"].as_array() else {
        return Value::Null;
    };
    let mut map = Map::new();
    for value in values {
        let key = if kind == "choice" {
            choice_key(&value["value"])
        } else {
            value["value"].as_u64().map(|index| index.to_string())
        };
        let Some(key) = key else {
            return Value::Null;
        };
        if map.insert(key, value["probability"].clone()).is_some() {
            return Value::Null;
        }
    }
    Value::Object(map)
}

pub(super) fn validate(definition: &Value, raw: &Value, warnings: &mut Vec<String>) -> Vec<String> {
    let mut errors = Vec::new();
    if !definition.is_object() || !definition["instructions"].is_string() {
        errors.push("Decision question requires original string instructions".into());
    }
    if definition.get("name").is_some_and(|name| !name.is_string()) {
        errors.push("Decision question name must be a string when supplied".into());
    }
    if !raw.is_object() {
        errors.push("Decision answer is missing or not an object".into());
        return errors;
    }
    if raw["name"] != definition["name"] {
        errors.push("Decision answer name differs from its ordered question".into());
    }
    if raw["type"] == "refusal" {
        errors.push("The model refused this question".into());
        return errors;
    }
    let kind = definition["type"].as_str().unwrap_or("");
    if raw["type"].as_str() != Some(kind) {
        errors.push("Decision answer type differs from its question".into());
    }
    if kind == "predicate" {
        if probability(&raw["probability"]).is_none() {
            errors
                .push("Predicate probability must be a finite number between zero and one".into());
        }
        return errors;
    }
    if !matches!(kind, "choice" | "score") {
        errors.push("Unknown Decision question type".into());
        return errors;
    }
    let mut expected = Vec::new();
    if kind == "choice" {
        if let Some(choices) = definition["choices"].as_array() {
            if !(2..=255).contains(&choices.len()) {
                errors.push("Decision Choice requires two to 255 unique typed options".into());
            }
            for choice in choices {
                if let Some(key) = choice_key(&choice["value"]) {
                    expected.push(key);
                } else {
                    errors.push("Decision Choice option must be a string or boolean".into());
                }
                if choice
                    .get("description")
                    .is_some_and(|value| !value.is_string())
                {
                    errors
                        .push("Decision Choice description must be a string when supplied".into());
                }
            }
        } else {
            errors.push("Decision Choice options are missing".into());
        }
    } else if let Some(levels) = definition["levels"].as_array() {
        // The reviewed HTTP reference provides no level-count limit. A
        // nonempty rubric is needed to interpret its zero-based score.
        if levels.is_empty() {
            errors.push("Decision Score requires a nonempty ordered rubric".into());
        }
        for (index, level) in levels.iter().enumerate() {
            expected.push(index.to_string());
            if !level["label"].is_string()
                || level.get("description").is_some_and(|v| !v.is_string())
            {
                errors.push(
                    "Decision Score levels require string labels and optional string descriptions"
                        .into(),
                );
            }
        }
    } else {
        errors.push("Decision Score levels are missing".into());
    }
    if expected.iter().collect::<BTreeSet<_>>().len() != expected.len() {
        errors.push("Decision options contain duplicate typed values".into());
    }
    let distribution = probabilities(raw, kind);
    let values = distribution
        .as_object()
        .filter(|map| map.len() == expected.len())
        .and_then(|map| {
            expected
                .iter()
                .map(|key| probability(&map[key]))
                .collect::<Option<Vec<_>>>()
        });
    if let Some(values) = values {
        if (values.iter().sum::<f64>() - 1.0).abs() > 0.0001 {
            errors.push("Decision probabilities do not sum to one".into());
        }
        if kind == "choice" {
            if let Some(index) = choice_key(&raw["choice"])
                .and_then(|key| expected.iter().position(|value| *value == key))
            {
                if values.iter().any(|value| *value > values[index] + 0.0001) {
                    warnings.push(
                        "Selected Decision option differs from the highest displayed probability; original values are preserved".into(),
                    );
                }
            } else {
                errors.push("Selected Decision option is absent from its typed domain".into());
            }
        } else {
            if let Some(score) = finite(&raw["score"])
                .filter(|score| *score >= 0.0 && *score <= expected.len().saturating_sub(1) as f64)
            {
                let weighted = values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| index as f64 * value)
                    .sum::<f64>();
                if (score - weighted).abs() > 0.001 {
                    warnings.push("Reported Score differs from the weighted displayed probabilities; original values are preserved".into());
                }
            } else {
                errors.push("Decision Score must be between zero and its last level index".into());
            }
            for item in raw["probabilities"].as_array().into_iter().flatten() {
                if item["value"]
                    .as_u64()
                    .and_then(|index| definition["levels"].get(index as usize))
                    .is_none_or(|level| level["label"] != item["label"])
                {
                    errors.push("Decision Score probability label differs from its level".into());
                }
            }
        }
    } else {
        errors.push(
            "Decision probability entries must cover the complete typed domain exactly once".into(),
        );
    }
    if probability(&raw["confidence"]).is_none() {
        errors.push("Decision confidence is missing or outside zero to one".into());
    }
    errors
}

pub(crate) fn typed_bin(definition: &Value, key: &str, count: Value) -> Option<Value> {
    let value = definition["choices"]
        .as_array()?
        .iter()
        .map(|option| &option["value"])
        .find(|value| choice_key(value).as_deref() == Some(key))?;
    let label = if value.is_boolean() {
        format!("{value} (boolean)")
    } else {
        serde_json::to_string(value).ok()?
    };
    Some(json!({"label":label,"key":key,"value":value,"count":count}))
}
