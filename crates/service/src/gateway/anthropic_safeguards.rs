use crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE;
use codexmanager_core::storage::UpstreamProvider;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

use super::runtime_config;

/// Claude Code sends this field when it asks the gateway to participate in
/// Anthropic's server-side safeguards/classifier flow.
pub(crate) const SAFEGUARDS_FIELD: &str = "safeguards";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SafeguardsCapability {
    Native,
    Emulated,
    Unsupported,
}

impl SafeguardsCapability {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Emulated => "emulated",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SafeguardToolUse {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) input: Value,
    pub(crate) truncated: bool,
}

pub(crate) trait SafeguardEvaluator: Send + Sync {
    fn evaluate(&self, tool_uses: &[SafeguardToolUse]) -> Value;
}

/// Return true when the request explicitly contains `safeguards`.
///
/// Presence is intentional here: even a null or otherwise malformed value must
/// not be silently removed by a protocol adapter.
pub(crate) fn request_has_safeguards(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .as_object()
                .map(|object| object.contains_key(SAFEGUARDS_FIELD))
        })
        .unwrap_or(false)
}

/// Return true when a native Anthropic response contains the classifier result
/// that must not be normalized into a generic gateway error body.
pub(crate) fn response_has_safeguard_results(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .as_object()
                .map(|object| object.contains_key("safeguard_results"))
        })
        .unwrap_or(false)
}

/// The gateway only advertises safeguards capability for a native Anthropic
/// Messages route. OpenAI/Codex-compatible upstreams are not assumed to
/// understand Anthropic's classifier contract.
pub(crate) fn safeguards_capability_for_route(
    protocol_type: &str,
    upstream_provider: UpstreamProvider,
) -> SafeguardsCapability {
    if protocol_type != PROTOCOL_ANTHROPIC_NATIVE {
        return SafeguardsCapability::Unsupported;
    }
    match upstream_provider {
        UpstreamProvider::Claude => SafeguardsCapability::Native,
        UpstreamProvider::Openai if runtime_config::claude_safeguards_emulation_enabled() => {
            SafeguardsCapability::Emulated
        }
        UpstreamProvider::Openai => SafeguardsCapability::Unsupported,
    }
}

pub(crate) fn safeguards_request_capability_kind(
    body: &[u8],
    protocol_type: &str,
    upstream_provider: UpstreamProvider,
) -> Option<SafeguardsCapability> {
    request_has_safeguards(body)
        .then(|| safeguards_capability_for_route(protocol_type, upstream_provider))
}

pub(crate) fn requested_dangerous_tool_use(safeguards: &Value) -> Option<Value> {
    safeguards.as_array()?.iter().find_map(|entry| {
        (entry.get("type").and_then(Value::as_str) == Some("dangerous_tool_use"))
            .then(|| entry.clone())
    })
}

pub(crate) fn available_results(tool_uses: BTreeMap<String, Value>) -> Value {
    json!([{
        "type": "dangerous_tool_use",
        "status": {
            "type": "available",
            "tool_uses": tool_uses
        }
    }])
}

pub(crate) fn unavailable_results(tool_uses: &[SafeguardToolUse], reason: &str) -> Value {
    let verdicts = tool_uses
        .iter()
        .map(|tool_use| {
            (
                tool_use.id.clone(),
                json!({
                    "type": "unavailable",
                    "reason": reason
                }),
            )
        })
        .collect::<Map<String, Value>>();
    json!([{
        "type": "dangerous_tool_use",
        "status": {
            "type": "available",
            "tool_uses": verdicts
        }
    }])
}

pub(crate) fn tool_uses_from_anthropic_message(value: &Value) -> Vec<SafeguardToolUse> {
    let Some(content) = value.get("content").and_then(Value::as_array) else {
        return Vec::new();
    };
    let hit_max_tokens = value.get("stop_reason").and_then(Value::as_str) == Some("max_tokens");
    content
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            (block.get("type").and_then(Value::as_str) == Some("tool_use")).then(|| {
                SafeguardToolUse {
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("toolu_unknown")
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string(),
                    input: block.get("input").cloned().unwrap_or_else(|| json!({})),
                    truncated: hit_max_tokens && index + 1 == content.len(),
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_safeguards_by_field_presence() {
        assert!(request_has_safeguards(
            br#"{"safeguards":{"enabled":true}}"#
        ));
        assert!(request_has_safeguards(br#"{"safeguards":null}"#));
        assert!(!request_has_safeguards(
            br#"{"metadata":{"safeguards":true}}"#
        ));
        assert!(!request_has_safeguards(br#"[]"#));
        assert!(response_has_safeguard_results(
            br#"{"safeguard_results":{"decision":"allow"}}"#
        ));
        assert!(!response_has_safeguard_results(br#"{"content":[]}"#));
    }

    #[test]
    fn only_native_claude_routes_have_capability() {
        assert_eq!(
            safeguards_capability_for_route(PROTOCOL_ANTHROPIC_NATIVE, UpstreamProvider::Claude,),
            SafeguardsCapability::Native
        );
        assert_eq!(
            safeguards_capability_for_route(PROTOCOL_ANTHROPIC_NATIVE, UpstreamProvider::Openai,),
            SafeguardsCapability::Emulated
        );
        assert_eq!(
            safeguards_capability_for_route("openai_compat", UpstreamProvider::Claude,),
            SafeguardsCapability::Unsupported
        );
    }

    #[test]
    fn gpt_6_astra_openai_route_uses_emulation() {
        assert_eq!(
            safeguards_request_capability_kind(
                br#"{"model":"gpt-6-astra","safeguards":{"enabled":true}}"#,
                PROTOCOL_ANTHROPIC_NATIVE,
                UpstreamProvider::Openai,
            ),
            Some(SafeguardsCapability::Emulated)
        );
    }
}
