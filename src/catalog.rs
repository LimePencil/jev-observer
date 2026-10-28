//! Offline, source-reviewed integration catalog. This never discovers providers
//! or makes inference calls; the configured upstream remains fixed.
use std::{collections::HashMap, sync::LazyLock};

use serde::Deserialize;
use serde_json::Value;

pub static CATALOG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("model_catalog.json")).expect("embedded model catalog")
});

// Normalization and import privacy checks select profiles for every record.
// Parse the fixed catalog once instead of cloning/deserializing its JSON on
// each lookup. Callers still receive their own value for protocol overrides.
static PROFILES: LazyLock<HashMap<&'static str, Profile>> = LazyLock::new(|| {
    CATALOG["models"]
        .as_array()
        .expect("catalog models")
        .iter()
        .map(|model| {
            (
                model["id"].as_str().expect("catalog model id"),
                serde_json::from_value(model["profile"].clone()).expect("catalog profile"),
            )
        })
        .collect()
});

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct Profile {
    #[serde(skip)]
    pub(crate) decisions: bool,
    pub probability_decimals: Option<i32>,
    pub score_min: usize,
    pub score_max: usize,
    pub choice_max: usize,
    pub choice_min: usize,
    pub box_answers: bool,
    pub box_presence: bool,
    pub nullable_noul_criteria: bool,
    pub json_legend: bool,
    pub nullable_score_levels: bool,
    pub reported_abstention: bool,
    pub modal_score: bool,
    pub list_choice: bool,
    pub structured_legend: bool,
    pub optional_instructions: bool,
    pub json_text: bool,
    pub optional_confidence: bool,
    pub optional_legend: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            decisions: false,
            probability_decimals: None,
            score_min: 2,
            score_max: 10,
            choice_max: 255,
            choice_min: 1,
            box_answers: false,
            box_presence: false,
            nullable_noul_criteria: false,
            json_legend: false,
            nullable_score_levels: false,
            reported_abstention: false,
            modal_score: false,
            list_choice: false,
            structured_legend: false,
            optional_instructions: false,
            json_text: false,
            optional_confidence: false,
            optional_legend: false,
        }
    }
}

pub fn profile(provider: &str) -> Profile {
    PROFILES.get(provider).cloned().unwrap_or_default()
}

pub fn known_profile(id: &str) -> bool {
    PROFILES.contains_key(id)
}
