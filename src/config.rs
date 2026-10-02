use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde_json::{Value, json};

use crate::{credentials::validate_key, model::NormalizeOptions};

/// A local observer for native TypeSafe requests. Credentials remain in memory.
#[derive(Clone, Parser)]
#[command(version, about)]
pub struct Config {
    /// Run with synthetic history in an isolated database; disables forwarding.
    #[arg(long)]
    pub demo: bool,
    /// HTTP port on 127.0.0.1.
    #[arg(long, default_value_t = 8765)]
    pub port: u16,
    /// Database location. Demo mode uses a separate *.demo.sqlite sibling.
    #[arg(long, default_value = ".jev-observer/observer.sqlite")]
    pub db: PathBuf,
    /// Fixed provider endpoint; request headers cannot override this URL.
    #[arg(long, default_value = "https://api.typesafe.ai/v1/systemone")]
    pub upstream: String,
    /// Optional fallback credential. Read from the environment so it cannot
    /// appear in process arguments.
    #[arg(skip)]
    pub api_key: Option<String>,
    /// Retain request state in local history. Disabled by default.
    #[arg(long)]
    pub capture_state: bool,
    /// Maximum captured bytes per request body and per response body.
    #[arg(long, default_value_t = 262_144)]
    pub capture_limit: usize,
    /// Maximum captures held across active requests, queue, and persistence.
    #[arg(long, default_value_t = 1024)]
    pub capture_slots: usize,
    #[arg(long, default_value_t = 1024)]
    pub queue_capacity: usize,
    #[arg(long, default_value_t = 7)]
    pub retention_days: u32,
    #[arg(long, default_value_t = 1_000_000)]
    pub max_records: usize,
    /// Explicit estimated USD rate; omitted rates produce unknown costs.
    #[arg(long)]
    pub input_price_per_million: Option<f64>,
    #[arg(long)]
    pub output_price_per_million: Option<f64>,
    /// Additional case-insensitive keys to redact from stored data.
    #[arg(long = "redact-key")]
    pub redact_keys: Vec<String>,
}

impl Config {
    pub fn validate(&mut self) -> Result<()> {
        let mut url = reqwest::Url::parse(&self.upstream).context("Invalid upstream URL")?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            bail!("Upstream must be an absolute HTTP or HTTPS URL");
        }
        if url.scheme() == "http" {
            let host = url.host_str().unwrap_or_default().trim_matches(['[', ']']);
            let loopback = host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback());
            if !loopback {
                bail!("HTTP upstream is allowed only on loopback; use HTTPS for remote providers");
            }
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!(
                "Upstream cannot contain credentials, a query, or a fragment; use TYPESAFE_API_KEY for credentials"
            );
        }
        if url.path() == "/" {
            url.set_path("/v1/systemone");
        }
        self.upstream = url.to_string();
        if self.demo {
            self.api_key = None;
        } else if self.api_key.is_none() {
            self.api_key = std::env::var("TYPESAFE_API_KEY").ok();
        }
        if self.capture_limit == 0 || self.capture_limit > 16 * 1024 * 1024 {
            bail!("--capture-limit must be between 1 and 16777216 bytes");
        }
        if self.capture_slots == 0 || self.capture_slots > 4096 {
            bail!("--capture-slots must be between 1 and 4096");
        }
        let budget = self
            .capture_slots
            .checked_mul(self.capture_limit)
            .and_then(|n| n.checked_mul(2));
        if budget.is_none_or(|n| n > 512 * 1024 * 1024) {
            bail!("Capture body budget (2 × slots × limit) must not exceed 512 MiB");
        }
        if self.queue_capacity == 0 || self.queue_capacity > 4096 {
            bail!("--queue-capacity must be between 1 and 4096");
        }
        if self.retention_days == 0
            || self.max_records == 0
            || i64::try_from(self.max_records).is_err()
        {
            bail!("Retention days and maximum records must be positive");
        }
        for rate in [self.input_price_per_million, self.output_price_per_million]
            .into_iter()
            .flatten()
        {
            if !rate.is_finite() || rate < 0.0 {
                bail!("Estimated token rates must be finite and nonnegative");
            }
        }
        if let Some(key) = &self.api_key {
            validate_key(key).context("Invalid TYPESAFE_API_KEY")?;
        }
        Ok(())
    }

    pub fn database_path(&self) -> PathBuf {
        if !self.demo {
            return self.db.clone();
        }
        let mut path = self.db.clone();
        let mut name = path.file_stem().unwrap_or_default().to_os_string();
        name.push(".demo.sqlite");
        path.set_file_name(name);
        path
    }

    pub fn normalize_options(&self) -> NormalizeOptions {
        NormalizeOptions {
            capture_state: self.capture_state,
            redact_keys: self.redact_keys.clone(),
            input_price_per_million: self.input_price_per_million,
            output_price_per_million: self.output_price_per_million,
        }
    }

    pub fn public_settings(&self) -> Value {
        json!({
            "demo": self.demo, "capture_state": self.capture_state,
            "retention_days": self.retention_days, "max_records": self.max_records,
            "capture_limit": self.capture_limit, "capture_slots": self.capture_slots,
            "queue_capacity": self.queue_capacity, "upstream": self.upstream,
            "version": env!("CARGO_PKG_VERSION"),
            "input_price_per_million": self.input_price_per_million,
            "output_price_per_million": self.output_price_per_million,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unbounded_capture_and_credential_urls() {
        for args in [
            vec![
                "observer",
                "--capture-limit",
                "16777216",
                "--capture-slots",
                "256",
            ],
            vec![
                "observer",
                "--upstream",
                "https://secret@example.com/v1/systemone",
            ],
            vec!["observer", "--upstream", "https://example.com/?key=secret"],
            vec!["observer", "--upstream", "http://example.com/v1/systemone"],
            vec!["observer", "--upstream", "http://192.168.1.9/v1/systemone"],
            vec!["observer", "--input-price-per-million", "NaN"],
        ] {
            assert!(Config::try_parse_from(args).unwrap().validate().is_err());
        }
    }

    #[test]
    fn demo_is_always_separate_and_settings_omit_secret() {
        let mut config =
            Config::try_parse_from(["observer", "--demo", "--db", "history.sqlite"]).unwrap();
        config.api_key = Some("hidden-key".into());
        assert_eq!(config.database_path(), PathBuf::from("history.demo.sqlite"));
        assert!(!config.public_settings().to_string().contains("hidden-key"));
        assert!(Config::try_parse_from(["observer", "--api-key", "hidden-key"]).is_err());
    }

    #[test]
    fn fallback_key_is_checked_before_startup() {
        for key in ["", "has space", "nonascii-é", "a\nnewline", "a\u{7f}b"] {
            let mut config = Config::try_parse_from(["observer"]).unwrap();
            config.api_key = Some(key.into());
            assert!(config.validate().is_err(), "accepted {key:?}");
        }
        let mut config = Config::try_parse_from(["observer"]).unwrap();
        config.api_key = Some("a".repeat(4097));
        assert!(config.validate().is_err());
    }
}
