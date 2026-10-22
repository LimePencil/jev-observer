//! Persistent history facade. Implementation modules keep connection, mutation,
//! query and export lifetimes separate without changing the public Store API.

use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

mod connection;
mod dashboard;
mod export;
mod filter;
mod groups;
mod metrics;
mod read;
mod retention;
mod value;
mod write;

pub use export::ExportSession;

#[derive(Clone)]
pub struct Store {
    path: Arc<PathBuf>,
    key: Option<Arc<String>>,
    retention_days: u32,
    max_records: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LabelOutcome {
    Saved,
    RequestNotFound,
    AnswerNotFound,
}

#[derive(Clone, Default, Deserialize)]
pub struct Filter {
    /// Internal snapshot time; callers cannot set it through HTTP query strings.
    #[serde(skip)]
    pub as_of: Option<i64>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub window: Option<String>,
    pub group: Option<String>,
    pub status: Option<String>,
    pub search: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub request_cursor: Option<String>,
    pub group_cursor: Option<String>,
    pub group_search: Option<String>,
}

// Preserve the existing private regression modules and their helper access.
#[cfg(test)]
use {
    anyhow::Result,
    chrono::Utc,
    connection::plaintext_database,
    dashboard::{SPARSE_ANSWER_QUERY, STRICT_ANSWER_QUERY},
    export::csv_cell,
    filter::FAILED,
    groups::VersionCounts,
    rusqlite::{Connection, OptionalExtension, params, params_from_iter},
    serde_json::{Value, json},
    std::{
        collections::{BTreeMap, HashMap},
        path::Path,
        time::Duration,
    },
    value::{serialize_without_field, text},
};

#[cfg(test)]
#[path = "label_provenance_tests.rs"]
mod label_provenance_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod dashboard_tests;

#[cfg(test)]
#[path = "group_detail_tests.rs"]
mod group_detail_tests;

#[cfg(test)]
#[path = "export_tests.rs"]
mod export_tests;

#[cfg(test)]
#[path = "retention_tests.rs"]
mod retention_tests;

#[cfg(test)]
#[path = "version_count_tests.rs"]
mod version_count_tests;
