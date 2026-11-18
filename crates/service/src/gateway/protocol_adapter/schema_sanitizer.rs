// Strict-schema normalization follows patterns documented in CLIProxyAPI
// (MIT), internal/util/claude_schema.go and internal/runtime/executor/helps/.
use serde_json::{Map, Value};

/// Strict OpenAI/Codex schema validators reject a few JavaScript-compatible
/// regular-expression constructs that Anthropic accepts verbatim. Claude Code's
/// built-in Artifact tool currently emits both `\0` and Unicode property
/// escapes, so forwarding that schema unchanged can reject the whole request
/// before the model sees it.
pub(crate) fn strip_unsupported_schema_patterns(schema: &mut Value) -> bool {
    fn walk(value: &mut Value) -> bool {
        match value {
            Value::Array(items) => items
                .iter_mut()
                .fold(false, |changed, item| walk(item) || changed),
            Value::Object(object) => {
                let mut changed = false;
                if object
                    .get("pattern")
                    .and_then(Value::as_str)
                    .is_some_and(unsupported_pattern)
                {
                    object.remove("pattern");
                    changed = true;
                }

                for (key, value) in object.iter_mut() {
                    // These entries are user data, not regular expressions.
                    if matches!(key.as_str(), "enum" | "const" | "default" | "examples") {
                        continue;
                    }
                    changed |= walk(value);
                }
                changed
            }
            _ => false,
        }
    }

    let unions_changed = flatten_root_object_unions(schema);
    walk(schema) || unions_changed
}

fn flatten_root_object_unions(schema: &mut Value) -> bool {
    let Some((union_key, variants)) = schema.as_object().and_then(|object| {
        ["anyOf", "oneOf", "allOf"].into_iter().find_map(|key| {
            object
                .get(key)
                .and_then(Value::as_array)
                .map(|items| (key, items.clone()))
        })
    }) else {
        return false;
    };
    if variants.is_empty() || !variants.iter().all(Value::is_object) {
        return false;
    }

    let mut merged = Map::new();
    let mut properties = Map::new();
    let mut required_sets = Vec::new();
    let mut any_variant_has_properties = false;
    for variant in &variants {
        let variant = variant
            .as_object()
            .expect("all variants checked as objects");
        if let Some(items) = variant.get("properties").and_then(Value::as_object) {
            any_variant_has_properties = true;
            for (name, value) in items {
                properties
                    .entry(name.clone())
                    .or_insert_with(|| value.clone());
            }
        }
        required_sets.push(
            variant
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<std::collections::BTreeSet<_>>(),
        );
        for (key, value) in variant {
            if matches!(
                key.as_str(),
                "properties" | "required" | "anyOf" | "oneOf" | "allOf"
            ) {
                continue;
            }
            merged.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if any_variant_has_properties {
        merged.insert("type".to_string(), Value::String("object".to_string()));
        merged.insert("properties".to_string(), Value::Object(properties));
    }
    let required = if union_key == "allOf" {
        required_sets
            .into_iter()
            .flatten()
            .collect::<std::collections::BTreeSet<_>>()
    } else {
        required_sets
            .split_first()
            .map(|(first, rest)| {
                first
                    .iter()
                    .filter(|item| rest.iter().all(|set| set.contains(*item)))
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>()
            })
            .unwrap_or_default()
    };
    if !required.is_empty() {
        merged.insert(
            "required".to_string(),
            Value::Array(required.into_iter().map(Value::String).collect()),
        );
    }
    let Some(object) = schema.as_object_mut() else {
        return false;
    };
    if let Some(description) = object.get("description").cloned() {
        merged.insert("description".to_string(), description);
    }
    if let Some(defs) = object.get("$defs").cloned() {
        merged.insert("$defs".to_string(), defs);
    }
    if merged.is_empty() {
        return false;
    }
    *object = merged;
    true
}

fn unsupported_pattern(pattern: &str) -> bool {
    let bytes = pattern.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        match bytes[index + 1] {
            b'0' => return true,
            b'p' | b'P' if bytes.get(index + 2) == Some(&b'{') => return true,
            _ => index += 2,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn removes_only_unsupported_patterns() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "pattern": "^[^\\0]*$"
                },
                "emoji": {
                    "type": "string",
                    "pattern": "^\\p{Emoji}+$"
                },
                "asset_id": {
                    "type": "string",
                    "pattern": "^[0-9a-f]{32}$"
                }
            }
        });

        assert!(strip_unsupported_schema_patterns(&mut schema));
        assert!(schema.pointer("/properties/path/pattern").is_none());
        assert!(schema.pointer("/properties/emoji/pattern").is_none());
        assert_eq!(
            schema
                .pointer("/properties/asset_id/pattern")
                .and_then(Value::as_str),
            Some("^[0-9a-f]{32}$")
        );
    }

    #[test]
    fn is_idempotent() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "pattern": "^[^\\0]*$"}
            }
        });
        assert!(strip_unsupported_schema_patterns(&mut schema));
        assert!(!strip_unsupported_schema_patterns(&mut schema));
    }

    #[test]
    fn flattens_root_object_unions_for_strict_function_tools() {
        let mut any_of = json!({
            "description": "choose a shape",
            "anyOf": [
                {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                },
                {
                    "type": "object",
                    "properties": {"url": {"type": "string"}},
                    "required": ["url", "path"]
                }
            ]
        });
        assert!(strip_unsupported_schema_patterns(&mut any_of));
        assert!(any_of.get("anyOf").is_none());
        assert_eq!(any_of["type"], "object");
        assert!(any_of.pointer("/properties/path").is_some());
        assert!(any_of.pointer("/properties/url").is_some());
        assert_eq!(any_of["required"], json!(["path"]));

        let mut all_of = json!({
            "allOf": [
                {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]},
                {"type": "object", "properties": {"b": {"type": "string"}}, "required": ["b"]}
            ]
        });
        assert!(strip_unsupported_schema_patterns(&mut all_of));
        assert_eq!(all_of["required"], json!(["a", "b"]));
        assert!(all_of.pointer("/properties/a").is_some());
        assert!(all_of.pointer("/properties/b").is_some());
    }
}
