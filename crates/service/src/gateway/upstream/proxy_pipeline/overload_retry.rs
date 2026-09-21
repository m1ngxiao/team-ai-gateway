//! Limits for the additional, pre-delivery model-overload failover path.
//! This does not extend the lifetime of a request or replay a committed stream.
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Default)]
pub(super) struct OverloadRetryBudget {
    used: bool,
}

impl OverloadRetryBudget {
    pub(super) fn used(&self) -> bool {
        self.used
    }

    pub(super) fn has_more_candidates(&self, pool_has_more: bool) -> bool {
        pool_has_more && !self.used
    }

    pub(super) fn permits(&self, attempted_accounts: usize, portable_request: bool) -> bool {
        !self.used && attempted_accounts == 1 && portable_request
    }

    pub(super) fn consume(&mut self) {
        self.used = true;
    }
}

pub(super) fn retry_delay(
    trace_id: &str,
    retry_after: Option<Duration>,
    request_deadline: Option<Instant>,
) -> Option<Duration> {
    // Spread concurrent retries without shared mutable state or a new RNG dependency.
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    trace_id.hash(&mut hash);
    let delay = Duration::from_millis(150 + hash.finish() % 101)
        .max(retry_after.unwrap_or_default());
    if delay > Duration::from_secs(2)
        || request_deadline.is_some_and(|deadline| {
            deadline.saturating_duration_since(Instant::now()) <= delay
        })
    {
        return None;
    }
    Some(delay)
}

/// Account-scoped stored/compacted history cannot be migrated without reconstructing
/// it. Server-executed tools may have side effects even before we see an output event.
/// Keep those requests on the existing path instead of silently discarding context.
pub(super) fn request_is_portable(body: &[u8]) -> bool {
    let Ok(Value::Object(body)) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    for field in ["previous_response_id", "conversation"] {
        if body.get(field).is_some_and(|value| !value.is_null()) {
            return false;
        }
    }
    if body.get("input").is_some_and(has_account_scoped_context)
        || body.get("messages").is_some_and(has_account_scoped_context)
    {
        return false;
    }
    match body.get("tools") {
        None | Some(Value::Null) => true,
        Some(Value::Array(tools)) => tools.iter().all(is_client_tool),
        _ => false,
    }
}

fn has_account_scoped_context(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key("encrypted_content")
                || object.get("file_id").is_some_and(|value| !value.is_null())
                || object.get("type").and_then(Value::as_str).is_some_and(|kind| {
                    matches!(kind, "item_reference" | "compaction" | "compaction_summary"
                        | "context_compaction" | "encrypted_content")
                })
                || object.values().any(has_account_scoped_context)
        }
        Value::Array(items) => items.iter().any(has_account_scoped_context),
        _ => false,
    }
}

fn is_client_tool(tool: &Value) -> bool {
    match tool.get("type").and_then(Value::as_str) {
        Some("function" | "custom") => true,
        Some("namespace") => tool.get("tools").and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().all(is_client_tool)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overload_stops_after_one_backup_even_when_the_pool_has_more_accounts() {
        let mut budget = OverloadRetryBudget::default();
        assert!(!budget.used());
        assert!(budget.permits(1, true));
        assert!(!budget.permits(2, true));
        assert!(!budget.permits(1, false));
        budget.consume();
        assert!(budget.used());
        assert!(!budget.permits(1, true));
        assert!(!budget.has_more_candidates(true));
    }

    #[test]
    fn retry_after_and_the_original_deadline_bound_backoff() {
        let delay = retry_delay("trace-one", None, None).unwrap();
        assert!((Duration::from_millis(150)..=Duration::from_millis(250)).contains(&delay));
        assert_eq!(retry_delay("trace-one", Some(Duration::from_secs(1)), None),
            Some(Duration::from_secs(1)));
        assert!(retry_delay("trace-one", Some(Duration::from_secs(3)), None).is_none());
        assert!(retry_delay("trace-one", None, Some(Instant::now())).is_none());
        assert!(retry_delay("trace-one", Some(Duration::from_secs(2)),
            Some(Instant::now() + Duration::from_secs(1))).is_none());
    }

    #[test]
    fn overload_retry_preserves_full_history_and_client_tool_results() {
        let body = serde_json::json!({"input":[
            {"role":"user","content":"hello"},
            {"type":"function_call","call_id":"call-test","name":"lookup","arguments":"{}"},
            {"type":"function_call_output","call_id":"call-test","output":"done"}
        ], "tools":[{"type":"function","name":"lookup"}]});
        assert!(request_is_portable(&serde_json::to_vec(&body).unwrap()));
    }

    #[test]
    fn overload_retry_does_not_replay_account_scoped_history_or_hosted_tools() {
        for body in [
            serde_json::json!({"previous_response_id":"resp-test","input":"next"}),
            serde_json::json!({"conversation":"conv-test","input":"next"}),
            serde_json::json!({"input":[{"type":"item_reference","id":"item-test"}]}),
            serde_json::json!({"input":[{"type":"reasoning","encrypted_content":"opaque-test"}]}),
            serde_json::json!({"input":[{"type":"compaction_summary","content":"test"}]}),
            serde_json::json!({"input":[{"role":"user","content":[{"type":"input_file","file_id":"file-test"}]}]}),
            serde_json::json!({"input":[{"role":"user","content":[{"type":"input_image","file_id":"file-test"}]}]}),
            serde_json::json!({"input":"go","tools":[{"type":"mcp"}]}),
            serde_json::json!({"input":"go","tools":[{"type":"namespace","tools":[{"type":"web_search"}]}]}),
        ] {
            assert!(!request_is_portable(&serde_json::to_vec(&body).unwrap()));
        }
        assert!(!request_is_portable(b"invalid-json"));
    }
}
