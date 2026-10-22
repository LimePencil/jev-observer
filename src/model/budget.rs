//! Independent byte and work accounting for atomic imports and credential redaction.

use super::types::answer_keys;
use anyhow::Result;
use serde_json::Value;

#[derive(Debug)]
pub struct ImportLimitError(pub(super) &'static str);

impl std::fmt::Display for ImportLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Import exceeds the {} limit", self.0)
    }
}

impl std::error::Error for ImportLimitError {}

#[derive(Clone, Copy)]
pub(super) struct ImportLimits {
    pub(super) record: usize,
    pub(super) batch: usize,
}

pub(super) const IMPORT_LIMITS: ImportLimits = ImportLimits {
    record: 16 * 1024 * 1024,
    batch: 64 * 1024 * 1024,
};

pub(super) fn limited_add(
    current: usize,
    additional: usize,
    limit: usize,
    kind: &'static str,
) -> Result<usize> {
    current
        .checked_add(additional)
        .filter(|total| *total <= limit)
        .ok_or_else(|| ImportLimitError(kind).into())
}

/// Count serialized bytes without allocating a second copy of the JSON.
pub(super) fn json_size<T: serde::Serialize + ?Sized>(
    value: &T,
    limit: usize,
    kind: &'static str,
) -> Result<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|total| *total <= self.limit)
                .ok_or_else(|| std::io::Error::other("JSON budget exceeded"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value).map_err(|_| ImportLimitError(kind))?;
    Ok(counter.bytes)
}

pub(super) struct ImportBudget {
    pub(super) limits: ImportLimits,
    pub(super) work: usize,
    pub(super) normalized: usize,
    pub(super) redaction_remaining: usize,
}

pub(super) struct ImportRecordBudget<'a> {
    pub(super) batch: &'a mut ImportBudget,
    pub(super) work: usize,
}

impl ImportRecordBudget<'_> {
    pub(super) fn charge(&mut self, bytes: usize) -> Result<()> {
        let record = limited_add(
            self.work,
            bytes,
            self.batch.limits.record,
            "normalization work",
        )?;
        let batch = limited_add(
            self.batch.work,
            bytes,
            self.batch.limits.batch,
            "normalization work",
        )?;
        self.work = record;
        self.batch.work = batch;
        Ok(())
    }

    pub(super) fn repeated(&mut self, bytes: usize, count: usize) -> Result<()> {
        let bytes = bytes
            .checked_mul(count)
            .ok_or(ImportLimitError("normalization work"))?;
        self.charge(bytes)
    }

    pub(super) fn size<T: serde::Serialize + ?Sized>(&self, value: &T) -> Result<usize> {
        json_size(value, self.batch.limits.record, "normalization work")
    }

    pub(super) fn value(&mut self, value: &Value) -> Result<()> {
        self.charge(self.size(value)?)
    }

    pub(super) fn structure(&mut self, record: &Value, format: &str) -> Result<()> {
        self.charge(512)?;
        if format == "systemone-capture" {
            for _ in answer_keys(&record["request"], &record["response"]) {
                self.charge(512)?;
            }
        } else if format == "observer-jsonl" {
            self.repeated(512, record["answers"].as_array().map_or(0, Vec::len))?;
            for key in ["actions", "labels"] {
                self.repeated(128, record[key].as_array().map_or(0, Vec::len))?;
            }
        }
        // Receipt answers are retained as provenance but never normalized into
        // inference answers, so their bytes are covered by the retained copies.
        Ok(())
    }

    pub(super) fn finish(&mut self, record: &Value) -> Result<()> {
        crate::normalized_json::check_value(record)?;
        let bytes = json_size(record, self.batch.limits.record, "normalized output")?;
        self.batch.normalized = limited_add(
            self.batch.normalized,
            bytes,
            self.batch.limits.batch,
            "normalized output",
        )?;
        Ok(())
    }
}
