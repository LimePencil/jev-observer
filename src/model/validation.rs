//! Provider-aware validation of native question and answer shapes.

use crate::catalog::Profile;
use serde_json::Map;
use serde_json::Value;

const PROBABILITY_TOLERANCE: f64 = 0.0001;

pub(super) fn finite(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| n.is_finite())
}

pub(super) fn probability(value: &Value) -> Option<f64> {
    finite(value).filter(|n| (0.0..=1.0).contains(n))
}

pub(super) fn token_count(value: &Value) -> Option<i64> {
    // SQLite's integer ledger is signed. A larger provider value must remain
    // unknown instead of disappearing at storage while still producing cost.
    value.as_i64().filter(|count| *count >= 0)
}

fn text_shape(value: &Value) -> bool {
    value.is_string() || value.is_object() || value.is_array()
}

pub(super) fn accepts_text(value: &Value, profile: &Profile, optional_null: bool) -> bool {
    text_shape(value) || profile.json_text || (optional_null && value.is_null())
}

pub(super) fn kind(definition: &Value, raw: &Value) -> &'static str {
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

pub(super) fn validate_answer(
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
        if !accepts_text(
            &definition["instructions"],
            profile,
            profile.optional_instructions,
        ) {
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
                            && accepts_text(value, profile, false)
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
