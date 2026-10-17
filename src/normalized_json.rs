//! Bounded parsing for Observer's stored and exported JSON envelopes.
//!
//! Raw provider bodies retain serde_json's default recursion guard. Observer
//! adds containers around their retained fields (and receipt provenance), so
//! its own representation needs a small, explicit allowance for those wrappers.
//! The default parser admits 127 containers; current capture/receipt envelopes
//! add at most two. A finite 144-container ceiling leaves room for these
//! wrappers while also bounding parsing, privacy traversal, cloning and drop.
//! Splitting records into SQL rows removes containers; request/export and
//! group/version views restore no more than the original envelope's depth.

use serde::Deserialize;
use serde_json::Value;

pub const MAX_DEPTH: usize = 144;

fn depth_error() -> serde_json::Error {
    serde::de::Error::custom("Observer JSON exceeds the nesting limit")
}

fn check_bytes(bytes: &[u8]) -> serde_json::Result<()> {
    let (mut depth, mut quoted, mut escaped) = (0_usize, false, false);
    for byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    if depth == MAX_DEPTH {
                        return Err(depth_error());
                    }
                    depth += 1;
                }
                // This pass only bounds depth. The JSON parser still checks
                // matching delimiters, strings, escapes and complete syntax.
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    Ok(())
}

pub fn from_slice(bytes: &[u8]) -> serde_json::Result<Value> {
    match serde_json::from_slice(bytes) {
        // Preserve the ordinary single-parse path. A successful default parse
        // already proves its depth is below our larger envelope allowance.
        Ok(value) => Ok(value),
        Err(error) if error.is_syntax() => {
            check_bytes(bytes)?;
            let mut parser = serde_json::Deserializer::from_slice(bytes);
            // This is the only extended parser. The nonrecursive preflight
            // above bounds recursive parsing, subsequent traversal and drop.
            parser.disable_recursion_limit();
            let value = Value::deserialize(&mut parser)?;
            parser.end()?;
            Ok(value)
        }
        Err(error) => Err(error),
    }
}

/// Check derived output before it can be saved. Iterators keep auxiliary
/// memory proportional to depth, even for objects or arrays with many entries.
pub fn check_value(value: &Value) -> serde_json::Result<()> {
    enum Children<'a> {
        Array(std::slice::Iter<'a, Value>),
        Object(serde_json::map::Values<'a>),
    }
    impl<'a> Children<'a> {
        fn of(value: &'a Value) -> Option<Self> {
            match value {
                Value::Array(array) => Some(Self::Array(array.iter())),
                Value::Object(object) => Some(Self::Object(object.values())),
                _ => None,
            }
        }
        fn next(&mut self) -> Option<&'a Value> {
            match self {
                Self::Array(iter) => iter.next(),
                Self::Object(iter) => iter.next(),
            }
        }
    }
    let Some(children) = Children::of(value) else {
        return Ok(());
    };
    let mut stack = vec![children];
    while let Some(children) = stack.last_mut() {
        if let Some(child) = children.next() {
            if let Some(children) = Children::of(child) {
                if stack.len() == MAX_DEPTH {
                    return Err(depth_error());
                }
                stack.push(children);
            }
        } else {
            stack.pop();
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "normalized_json_tests.rs"]
mod tests;
