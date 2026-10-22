//! Capture identities, wire snapshots, and normalization preferences.

use super::identity::fingerprint;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Identity provenance travels with a capture when its buffers move into the
/// collector. Only these constructors can mark locally generated identities;
/// caller-supplied UUIDs or fingerprint-looking strings remain untrusted.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(transparent)]
pub struct CaptureId {
    value: String,
    #[serde(skip)]
    pub(super) generated: bool,
}

impl CaptureId {
    pub fn new_live() -> Self {
        Self {
            value: uuid::Uuid::now_v7().to_string(),
            generated: true,
        }
    }

    pub(super) fn raw_import(source: &str, provider: &str, event_id: &str) -> Self {
        Self {
            value: fingerprint(
                "capture_v1",
                &json!([source, provider.to_ascii_lowercase(), event_id]),
            ),
            generated: true,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl From<String> for CaptureId {
    fn from(value: String) -> Self {
        Self {
            value,
            generated: false,
        }
    }
}

impl From<&str> for CaptureId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl std::ops::Deref for CaptureId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Capture {
    pub id: CaptureId,
    pub timestamp: i64,
    pub source: String,
    pub task_version: Option<String>,
    pub adapter: Option<String>,
    pub status: u16,
    pub duration_ms: f64,
    pub request: Vec<u8>,
    pub response: Vec<u8>,
    /// Bounded live transport metadata; imports and legacy fixtures use None.
    pub transport: Option<super::capture_transport::CaptureTransport>,
    pub capture_complete: bool,
    pub transport_error: Option<String>,
    pub secret: Option<String>,
    // A local client token or accepted dashboard Basic header. These auth
    // paths are mutually exclusive; this value is used only for redaction.
    pub local_token: Option<String>,
    pub access_token: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct NormalizeOptions {
    pub provider: Option<String>,
    pub model_profile: Option<String>,
    pub protocol: Protocol,
    pub capture_state: bool,
    pub redact_keys: Vec<String>,
    pub input_price_per_million: Option<f64>,
    pub output_price_per_million: Option<f64>,
}

impl NormalizeOptions {
    pub fn validation_profile(&self) -> crate::catalog::Profile {
        let mut profile = crate::catalog::profile(self.profile_name());
        profile.decisions = self.protocol == Protocol::OpenaiDecisions;
        profile
    }
    pub fn profile_name(&self) -> &str {
        self.model_profile
            .as_deref()
            .or(self.provider.as_deref())
            .unwrap_or("typesafe")
    }
}

/// Wire format and model validation are independent of credential attribution.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Protocol {
    #[default]
    Systemone,
    OpenaiDecisions,
    CloudflareSystemone,
}

impl Protocol {
    pub fn local_path(self) -> &'static str {
        match self {
            Self::OpenaiDecisions => "/v1/decisions",
            Self::Systemone | Self::CloudflareSystemone => "/v1/systemone",
        }
    }
}

pub(super) fn answer_keys<'a>(
    request: &'a Value,
    response: &'a Value,
) -> impl Iterator<Item = &'a String> {
    let questions = request["questions"].as_object();
    questions.into_iter().flat_map(|map| map.keys()).chain(
        response["answers"]
            .as_object()
            .into_iter()
            .flat_map(|map| map.keys())
            .filter(move |key| questions.is_none_or(|map| !map.contains_key(*key))),
    )
}
