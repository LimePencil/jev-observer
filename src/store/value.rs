use anyhow::Result;
use serde::{Serialize, ser::SerializeMap};
use serde_json::Value;

pub(super) fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
pub(super) fn optional_text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str()
}
pub(super) fn parse(s: String) -> rusqlite::Result<Value> {
    crate::normalized_json::from_slice(s.as_bytes()).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })
}

pub(super) fn label_fields(annotation: &Value) -> Option<(&str, &str)> {
    let key = annotation["key"]
        .as_str()
        .or(annotation["question_key"].as_str())?;
    let label = annotation["label"].as_str()?;
    ["correct", "incorrect", "unknown"]
        .contains(&label)
        .then_some((key, label))
}

pub(super) fn serialize_without_field(
    fields: &serde_json::Map<String, Value>,
    excluded: &str,
) -> serde_json::Result<String> {
    struct WithoutField<'a> {
        fields: &'a serde_json::Map<String, Value>,
        excluded: &'a str,
    }
    impl Serialize for WithoutField<'_> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let removed = self.fields.contains_key(self.excluded);
            let mut output =
                serializer.serialize_map(Some(self.fields.len() - usize::from(removed)))?;
            let last = self.fields.iter().next_back();
            for (key, value) in self.fields {
                if removed && key == self.excluded {
                    // preserve_order Map::remove moves the last entry into
                    // the removed slot. Keep those exact bytes while borrowing
                    // the surviving fields instead of cloning discarded data.
                    if let Some((last_key, last_value)) = last
                        && last_key != key
                    {
                        output.serialize_entry(last_key, last_value)?;
                    }
                } else if !removed || !last.is_some_and(|(last_key, _)| last_key == key) {
                    output.serialize_entry(key, value)?;
                }
            }
            output.end()
        }
    }
    serde_json::to_string(&WithoutField { fields, excluded })
}
