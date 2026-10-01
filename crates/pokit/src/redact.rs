//! Masking of secret text in everything pokit emits.

use serde_json::Value;

pub const MASK: &str = "***";

/// Replaces every occurrence of every non-empty secret, in every string and key of `value`, with `***`.
pub fn redact(value: &mut Value, secrets: &[String]) {
    let mut secrets: Vec<&str> = secrets
        .iter()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .collect();
    if secrets.is_empty() {
        return;
    }
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    walk(value, &secrets);
}

fn mask(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        out = out.replace(s, MASK);
    }
    out
}

fn walk(value: &mut Value, secrets: &[&str]) {
    match value {
        Value::String(s) => *s = mask(s, secrets),
        Value::Array(items) => items.iter_mut().for_each(|v| walk(v, secrets)),
        Value::Object(map) => {
            let entries = std::mem::take(map);
            for (k, mut v) in entries {
                walk(&mut v, secrets);
                map.insert(mask(&k, secrets), v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_secret_inside_any_string_becomes_stars() {
        let mut v = json!({ "text": "submit name=kim password=hunter2", "n": 3 });
        redact(&mut v, &["hunter2".to_string()]);
        assert_eq!(v, json!({ "text": "submit name=kim password=***", "n": 3 }));
    }

    #[test]
    fn nested_arrays_and_objects_are_masked_too() {
        let mut v = json!({ "entries": [{ "text": "a hunter2 b hunter2" }], "args": ["hunter2"] });
        redact(&mut v, &["hunter2".to_string()]);
        assert_eq!(
            v,
            json!({ "entries": [{ "text": "a *** b ***" }], "args": ["***"] })
        );
    }

    #[test]
    fn object_keys_are_masked() {
        let mut v = json!({ "hunter2": 1 });
        redact(&mut v, &["hunter2".to_string()]);
        assert_eq!(v, json!({ "***": 1 }));
    }

    #[test]
    fn an_empty_secret_masks_nothing() {
        let mut v = json!({ "text": "abc" });
        redact(&mut v, &[String::new()]);
        assert_eq!(v, json!({ "text": "abc" }));
    }

    #[test]
    fn the_longer_of_two_overlapping_secrets_is_masked_whole() {
        let mut v = json!("pass and password1");
        redact(&mut v, &["pass".to_string(), "password1".to_string()]);
        assert_eq!(v, json!("*** and ***"));
    }
}
