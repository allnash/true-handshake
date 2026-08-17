//! Canonical JSON (RFC 8785 / JCS), restricted to the value space True Handshake
//! actually uses.
//!
//! Why this exists: a receipt is only verifiable if an independent implementation
//! — written from the published spec, in another language, by someone who does not
//! trust us — computes byte-identical hashes. That requires deterministic key
//! ordering and deterministic number formatting.
//!
//! We deliberately reject non-integer numbers rather than implementing the
//! ECMAScript number-formatting algorithm. Every amount in this system is already
//! integer minor units, so the restriction costs nothing and removes the single
//! most error-prone part of JCS.

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::DomainError;

/// Serialize a value to canonical JSON bytes.
pub fn to_canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, DomainError> {
    let v = serde_json::to_value(value)
        .map_err(|e| DomainError::Invalid(format!("value is not serializable: {e}")))?;
    let mut out = String::new();
    write_value(&v, &mut out)?;
    Ok(out.into_bytes())
}

/// Canonical JSON as a string (handy for debugging and for the published spec's
/// worked examples).
pub fn to_canonical_string<T: Serialize>(value: &T) -> Result<String, DomainError> {
    Ok(String::from_utf8(to_canonical_bytes(value)?).expect("canonical JSON is valid UTF-8"))
}

/// SHA-256 over the canonical encoding, lowercase hex.
pub fn canonical_hash<T: Serialize>(value: &T) -> Result<String, DomainError> {
    let bytes = to_canonical_bytes(value)?;
    Ok(hex(&Sha256::digest(&bytes)))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn write_value(v: &serde_json::Value, out: &mut String) -> Result<(), DomainError> {
    match v {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(true) => out.push_str("true"),
        serde_json::Value::Bool(false) => out.push_str("false"),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                return Err(DomainError::NonIntegerNumber(n.to_string()));
            }
        }
        serde_json::Value::String(s) => write_string(s, out),
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            // JCS orders members by the UTF-16 code units of their names.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by_key(|k| utf16_units(k));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                write_value(&map[*k], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn utf16_units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys_and_strips_whitespace() {
        let v = json!({ "b": 1, "a": 2, "C": 3 });
        // Uppercase sorts before lowercase in UTF-16 code-unit order.
        assert_eq!(to_canonical_string(&v).unwrap(), r#"{"C":3,"a":2,"b":1}"#);
    }

    #[test]
    fn key_order_is_insertion_independent() {
        let a = json!({ "z": 1, "m": 2, "a": 3 });
        let b = json!({ "a": 3, "z": 1, "m": 2 });
        assert_eq!(to_canonical_string(&a).unwrap(), to_canonical_string(&b).unwrap());
    }

    #[test]
    fn rejects_floats() {
        let v = json!({ "amount": 40.5 });
        assert!(matches!(
            to_canonical_bytes(&v),
            Err(DomainError::NonIntegerNumber(_))
        ));
    }

    #[test]
    fn escapes_control_characters() {
        let v = json!({ "s": "line\nbreak\u{1}" });
        assert_eq!(
            to_canonical_string(&v).unwrap(),
            r#"{"s":"line\nbreak\u0001"}"#
        );
    }

    #[test]
    fn nested_objects_sort_too() {
        let v = json!({ "outer": { "b": [ {"y": 1, "x": 2} ], "a": 1 } });
        assert_eq!(
            to_canonical_string(&v).unwrap(),
            r#"{"outer":{"a":1,"b":[{"x":2,"y":1}]}}"#
        );
    }
}
