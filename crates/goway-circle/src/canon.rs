//! Canonical JSON (RFC 8785 subset) so every platform hashes the same bytes.

use serde_json::Value;

/// Encode `value` with sorted object keys and no whitespace.
///
/// Only strings, arrays, objects, booleans and null are used by roster
/// operations, so number canonicalization is not needed (and is refused).
pub(crate) fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        Value::Number(_) => unreachable!("roster operations carry no numbers"),
        other => out.push_str(&other.to_string()),
    }
}
