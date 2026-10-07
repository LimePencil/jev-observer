//! Offline, source-reviewed integration catalog. This never discovers providers
//! or makes inference calls; the configured upstream remains fixed.
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::Value;

pub static CATALOG: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("model_catalog.json")).expect("embedded model catalog")
});

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub probability_decimals: Option<i32>,
    pub score_min: usize,
    pub score_max: usize,
    pub choice_max: usize,
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
            probability_decimals: None,
            score_min: 2,
            score_max: 10,
            choice_max: 255,
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
    CATALOG["models"]
        .as_array()
        .expect("catalog models")
        .iter()
        .find(|model| model["id"].as_str() == Some(provider))
        .map(|model| serde_json::from_value(model["profile"].clone()).expect("catalog profile"))
        .unwrap_or_default()
}
