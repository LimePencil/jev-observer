//! Local normalization. No inference, network requests, or model-derived labels.
//!
//! Native types: https://docs.typesafe.ai/api (reviewed 2026-09-22).
//! jgrep-v1 recognizes only the literal prefix in the reviewed buildRequest:
//! https://github.com/kyu1204/jgrep/blob/e18f36bdf4fb24e60ac43c7435ef54b0997f383c/src/jgrep.ts
//! Receipt adapter: JevRouter f944acb6530621bced023352e2358a63218bf4d9,
//! src/types.ts RouteResult and src/store.ts saveDecision. Receipts do not
//! contain the original complete question definitions or necessarily timings.

// The facade stays stable for the collector, server and storage fixtures.
// Imports share the same record builder as live captures; byte/work accounting
// and privacy filtering remain independent of the individual import formats.
// Provider shapes are validated before any derived observation is accepted.

mod budget;
pub(crate) mod decisions;
mod identity;
mod imports;
mod normalize;
mod privacy;
mod samples;
mod types;
mod validation;

#[cfg(test)]
#[path = "model/tests/omni.rs"]
mod omni_tests;

pub use budget::ImportLimitError;
pub use imports::{
    IMPORT_IDENTITY_ERROR_MESSAGE, ImportIdentityError, import_records, valid_timestamp,
};
pub use normalize::normalize;
pub use privacy::{
    IMPORT_REDACTION_COLLISION_MESSAGE, IMPORT_REDACTION_VALIDATION_MESSAGE,
    RedactionCollisionError, RedactionValidationError,
};
pub use samples::sample_records;
pub use types::{Capture, CaptureId, NormalizeOptions, Protocol};

#[cfg(test)]
use privacy::{
    INPUT_FIELDS, IdentifierNamespace, REDACTED, REDACTION_COLLISION_MESSAGE,
    REDACTION_LIMIT_MESSAGE, REDACTION_LIMITS, REDACTION_VALIDATION_MESSAGE, RedactionLimits,
    SecretMatcher, SemanticIdentifiers, clear_input_fields, collect_secret_string,
    ordered_unique_secrets, privacy, privacy_capture_envelope, privacy_observer,
    privacy_with_input_filter, scrub_string,
};

#[cfg(test)]
use identity::{fingerprint, group_fingerprint};

#[cfg(test)]
use budget::{IMPORT_LIMITS, ImportBudget, ImportLimits, ImportRecordBudget, json_size};
#[cfg(test)]
use imports::{MAX_IMPORT_RECORDS, import_capture, import_records_with_limits};

#[cfg(test)]
use anyhow::Result;

#[cfg(test)]
use serde_json::Map;

#[cfg(test)]
use serde_json::Value;

#[cfg(test)]
use serde_json::json;

#[cfg(test)]
use std::collections::BTreeSet;

#[cfg(test)]
use crate::catalog;

#[cfg(test)]
#[path = "model/tests/unit.rs"]
mod tests;

#[cfg(test)]
#[path = "model/tests/compatibility_tests.rs"]
mod compatibility_tests;

#[cfg(test)]
#[path = "model/tests/import_budget_tests.rs"]
mod import_budget_tests;

#[cfg(test)]
#[path = "model/tests/import_identity_tests.rs"]
mod import_identity_tests;

#[cfg(test)]
#[path = "model/tests/redaction_tests.rs"]
mod redaction_tests;

#[cfg(test)]
#[path = "model/tests/envelope_privacy_tests.rs"]
mod envelope_privacy_tests;

#[cfg(test)]
#[path = "model/tests/generated_identity_tests.rs"]
mod generated_identity_tests;

#[cfg(test)]
#[path = "model/tests/redaction_collision_tests.rs"]
mod redaction_collision_tests;

#[cfg(test)]
#[path = "model/tests/redaction_validation_tests.rs"]
mod redaction_validation_tests;

#[cfg(test)]
#[path = "model/tests/semantic_identifier_tests.rs"]
mod semantic_identifier_tests;

#[cfg(test)]
#[path = "model/tests/semantic_header_tests.rs"]
mod semantic_header_tests;

#[cfg(test)]
#[path = "model/tests/receipt_privacy_tests.rs"]
mod receipt_privacy_tests;

#[cfg(test)]
#[path = "model/tests/basic_credential_tests.rs"]
mod basic_credential_tests;

#[cfg(test)]
#[path = "model/tests/input_opt_out_tests.rs"]
mod input_opt_out_tests;
