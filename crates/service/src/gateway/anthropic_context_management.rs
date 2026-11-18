// Claude Code context-management compatibility behavior is adapted from
// LiteLLM (MIT), commit 56e60d3b7d4b472b0c641d47202369c3dc3ad9c8:
// litellm/llms/anthropic/messages/context_management/.
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const CLEAR_TOOL_USES_EDIT_TYPE: &str = "clear_tool_uses_20250919";
pub(crate) const COMPACT_EDIT_TYPE: &str = "compact_20260112";
pub(crate) const DEFAULT_INPUT_TOKENS_TRIGGER: i64 = 100_000;
pub(crate) const DEFAULT_KEEP_TOOL_USES: i64 = 3;
pub(crate) const DEFAULT_COMPACT_TRIGGER_TOKENS: i64 = 150_000;
pub(crate) const MIN_COMPACT_TRIGGER_TOKENS: i64 = 50_000;
pub(crate) const CLEARED_TOOL_RESULT_PLACEHOLDER: &str = "[Cleared by context management]";
pub(crate) const COMPACT_DEFAULT_INSTRUCTIONS: &str =
    "You have written a partial transcript for the initial task above. Please write a summary of the transcript. The purpose of this summary is to provide continuity so you can continue to make progress towards solving the task in a future context, where the raw history above may not be accessible and will be replaced with this summary. Write down anything that would be helpful, including the state, next steps, learnings etc. You must wrap your summary in a <summary></summary> block.";
pub(crate) const COMPACT_NO_TOOL_CALLS_SUFFIX: &str =
    " Do not call any tools while writing this summary; respond with text only.";

const TINY_RESULT_BYTES: usize = 46;
const TRANSCRIPT_HEAD_CHARS: usize = 80_000;
const TRANSCRIPT_TAIL_CHARS: usize = 320_000;
const TRANSCRIPT_LINE_CHARS: usize = 8_000;

#[derive(Debug, Clone)]
pub(crate) struct CompactionResult {
    pub(crate) summary: String,
    pub(crate) input_tokens: i64,
    pub(crate) output_tokens: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct CompactRequest {
    pub(crate) trigger_tokens: i64,
    pub(crate) current_tokens: i64,
    pub(crate) transcript: String,
    pub(crate) instructions: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ContextManagementState {
    requested: bool,
    applied_edits: Vec<Value>,
    compact_request: Option<CompactRequest>,
    compact_edit: Option<Value>,
    compaction: Option<CompactionResult>,
}

impl ContextManagementState {
    pub(crate) fn compact_request(&self) -> Option<&CompactRequest> {
        self.compact_request.as_ref()
    }

    pub(crate) fn needs_compaction_summary(&self) -> bool {
        self.compact_request.is_some() && self.compaction.is_none()
    }

    pub(crate) fn record_compaction(&mut self, result: CompactionResult) {
        if let Some(edit) = self.compact_edit.as_mut().and_then(Value::as_object_mut) {
            edit.remove("error");
            edit.insert(
                "summary_input_tokens".to_string(),
                Value::from(result.input_tokens.max(0)),
            );
            edit.insert(
                "summary_output_tokens".to_string(),
                Value::from(result.output_tokens.max(0)),
            );
        }
        self.compaction = Some(result);
    }

    pub(crate) fn record_compaction_error(&mut self, error: &str) {
        if let Some(edit) = self.compact_edit.as_mut().and_then(Value::as_object_mut) {
            edit.insert("error".to_string(), Value::String(error.to_string()));
        }
    }

    pub(crate) fn response_enrichment(&self) -> Option<AnthropicContextEnrichment> {
        if !self.requested {
            return None;
        }
        let mut applied_edits = self.applied_edits.clone();
        if let Some(edit) = self.compact_edit.as_ref() {
            let has_result = self.compaction.is_some();
            let has_error = edit.get("error").is_some();
            let has_warnings = edit
                .get("warnings")
                .and_then(Value::as_array)
                .is_some_and(|warnings| !warnings.is_empty());
            if has_result || has_error || has_warnings {
                applied_edits.push(edit.clone());
            }
        }
        let compaction = self.compaction.as_ref().map(|result| CompactionBlock {
            summary: result.summary.clone(),
            input_tokens: result.input_tokens.max(0),
            output_tokens: result.output_tokens.max(0),
        });
        (!applied_edits.is_empty() || compaction.is_some()).then(|| AnthropicContextEnrichment {
            applied_edits,
            compaction,
        })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompactionBlock {
    pub(crate) summary: String,
    pub(crate) input_tokens: i64,
    pub(crate) output_tokens: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct AnthropicContextEnrichment {
    pub(crate) applied_edits: Vec<Value>,
    pub(crate) compaction: Option<CompactionBlock>,
}

impl AnthropicContextEnrichment {
    pub(crate) fn context_management_value(&self) -> Option<Value> {
        (!self.applied_edits.is_empty()).then(|| {
            json!({
                "applied_edits": self.applied_edits
            })
        })
    }

    pub(crate) fn compaction_value(&self) -> Option<Value> {
        self.compaction.as_ref().map(|compaction| {
            json!({
                "type": "compaction",
                "content": compaction.summary
            })
        })
    }

    pub(crate) fn compaction_usage_value(&self) -> Option<Value> {
        self.compaction.as_ref().map(|compaction| {
            json!({
                "type": "compaction",
                "input_tokens": compaction.input_tokens,
                "output_tokens": compaction.output_tokens
            })
        })
    }
}

/// Prepare an Anthropic Messages request for a non-Anthropic upstream.
///
/// The returned state must travel with the request to the response bridge. The
/// request body no longer contains the private `context_management` field:
/// clear-tool edits are applied locally and compaction is completed by the
/// upstream summary sub-call before the main request is sent.
pub(crate) fn prepare_context_management(
    body: &mut Value,
) -> Result<ContextManagementState, String> {
    let Some(object) = body.as_object_mut() else {
        return Err("anthropic request body must be an object".to_string());
    };
    let Some(spec) = object.remove("context_management") else {
        return Ok(ContextManagementState::default());
    };
    let edits = normalize_edits(&spec);
    let mut state = ContextManagementState {
        requested: true,
        ..Default::default()
    };
    for edit in edits {
        match edit.get("type").and_then(Value::as_str) {
            Some(CLEAR_TOOL_USES_EDIT_TYPE) => {
                if let Some(applied) = apply_clear_tool_uses(body, &edit)? {
                    state.applied_edits.push(applied);
                }
            }
            Some(COMPACT_EDIT_TYPE) => {
                apply_compact_plan(body, &edit, &mut state)?;
            }
            Some(unknown) => {
                log::debug!(
                    "event=anthropic_context_management_unknown_edit edit_type={}",
                    unknown
                );
            }
            None => {}
        }
    }
    Ok(state)
}

fn normalize_edits(spec: &Value) -> Vec<Map<String, Value>> {
    let raw_edits = match spec {
        Value::Array(items) => items.as_slice(),
        Value::Object(object) => object
            .get("edits")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        _ => &[],
    };
    raw_edits
        .iter()
        .filter_map(|edit| match edit {
            Value::String(edit_type) if !edit_type.trim().is_empty() => Some(Map::from_iter([(
                "type".to_string(),
                Value::String(edit_type.trim().to_string()),
            )])),
            Value::Object(object)
                if object.get("type").and_then(Value::as_str) == Some("compaction") =>
            {
                let mut mapped = Map::new();
                mapped.insert(
                    "type".to_string(),
                    Value::String(COMPACT_EDIT_TYPE.to_string()),
                );
                if let Some(threshold) = object.get("compact_threshold").and_then(Value::as_i64) {
                    mapped.insert(
                        "trigger".to_string(),
                        json!({"type": "input_tokens", "value": threshold}),
                    );
                }
                for (key, value) in object {
                    if !matches!(key.as_str(), "type" | "compact_threshold") {
                        mapped.insert(key.clone(), value.clone());
                    }
                }
                Some(mapped)
            }
            Value::Object(object) => Some(object.clone()),
            _ => None,
        })
        .collect()
}

fn apply_clear_tool_uses(
    body: &mut Value,
    edit: &Map<String, Value>,
) -> Result<Option<Value>, String> {
    let current_tokens = estimate_value_tokens(body);
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(None);
    };
    let messages_before = Value::Array(messages.clone());
    let exclude_tools = string_set(edit.get("exclude_tools"));
    let input_tools = ClearToolInputs::parse(edit.get("clear_tool_inputs"));
    let clear_at_least = edit
        .get("clear_at_least")
        .and_then(Value::as_object)
        .and_then(|value| value.get("value"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let trigger = edit.get("trigger").and_then(Value::as_object);
    let trigger_type = trigger
        .and_then(|trigger| trigger.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("input_tokens");
    let trigger_value = trigger
        .and_then(|trigger| trigger.get("value"))
        .and_then(Value::as_i64);
    let keep = edit
        .get("keep")
        .and_then(Value::as_object)
        .and_then(|value| value.get("value"))
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_KEEP_TOOL_USES)
        .max(0);

    let results = tool_result_contents(messages);
    let tiny = results
        .iter()
        .filter(|(_, content)| is_tiny_result(content))
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    let uses = tool_uses_in_order(messages);
    let clearable = uses
        .iter()
        .filter(|(id, name)| {
            !exclude_tools.contains(name) && (!tiny.contains(id) || input_tools.clears(name))
        })
        .cloned()
        .collect::<Vec<_>>();

    let trigger_met = match trigger_type {
        "tool_uses" => trigger_value
            .map(|value| clearable.len() as i64 > value)
            .unwrap_or(false),
        _ => current_tokens > trigger_value.unwrap_or(DEFAULT_INPUT_TOKENS_TRIGGER),
    };
    if !trigger_met || clearable.len() as i64 <= keep {
        return Ok(None);
    }

    let last_completed = last_tool_result_id(messages);
    let candidates = clearable[..clearable.len() - keep as usize]
        .iter()
        .filter(|(id, _)| {
            results.contains_key(id) && Some(id.as_str()) != last_completed.as_deref()
        })
        .cloned()
        .collect::<Vec<_>>();
    let selected = select_clear_batch(
        &candidates,
        &results,
        messages,
        &input_tools,
        clear_at_least,
    );
    if selected.is_empty() {
        return Ok(None);
    }
    let cleared_count = clear_selected(messages, &selected, &input_tools);
    if cleared_count == 0 {
        return Ok(None);
    }
    let tokens_before_messages = estimate_value_tokens(&messages_before);
    let tokens_after = estimate_value_tokens(&Value::Array(messages.to_vec()));
    Ok(Some(json!({
        "type": CLEAR_TOOL_USES_EDIT_TYPE,
        "cleared_tool_uses": cleared_count,
        "cleared_input_tokens": (tokens_before_messages - tokens_after).max(0)
    })))
}

fn apply_compact_plan(
    body: &Value,
    edit: &Map<String, Value>,
    state: &mut ContextManagementState,
) -> Result<(), String> {
    let mut warnings = Vec::new();
    let trigger = edit.get("trigger").and_then(Value::as_object);
    let trigger_type = trigger
        .and_then(|trigger| trigger.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("input_tokens");
    if trigger_type != "input_tokens" {
        warnings.push(format!(
            "unsupported_trigger_type_{trigger_type}_using_input_tokens"
        ));
    }
    let trigger_tokens = match trigger
        .and_then(|trigger| trigger.get("value"))
        .and_then(Value::as_i64)
    {
        Some(value) if value < MIN_COMPACT_TRIGGER_TOKENS => {
            return Err(format!(
                "context_management.compact_20260112.trigger.value must be at least {MIN_COMPACT_TRIGGER_TOKENS} tokens"
            ));
        }
        Some(value) => value,
        None => DEFAULT_COMPACT_TRIGGER_TOKENS,
    };
    if edit
        .get("pause_after_compaction")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        warnings.push("pause_after_compaction_ignored".to_string());
    }
    let mut compact_edit = Map::from_iter([(
        "type".to_string(),
        Value::String(COMPACT_EDIT_TYPE.to_string()),
    )]);
    if !warnings.is_empty() {
        compact_edit.insert("warnings".to_string(), json!(warnings));
    }
    state.compact_edit = Some(Value::Object(compact_edit));

    let current_tokens = estimate_value_tokens(body);
    if current_tokens <= trigger_tokens {
        return Ok(());
    }
    let has_tools = body
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty());
    let instructions = edit
        .get("instructions")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            let mut prompt = COMPACT_DEFAULT_INSTRUCTIONS.to_string();
            if has_tools {
                prompt.push_str(COMPACT_NO_TOOL_CALLS_SUFFIX);
            }
            prompt
        });
    state.compact_request = Some(CompactRequest {
        trigger_tokens,
        current_tokens,
        transcript: render_transcript(body),
        instructions,
    });
    Ok(())
}

#[derive(Debug, Clone)]
enum ClearToolInputs {
    None,
    All,
    Selected(BTreeSet<String>),
}

impl ClearToolInputs {
    fn parse(value: Option<&Value>) -> Self {
        match value {
            Some(Value::Bool(true)) => Self::All,
            Some(Value::Array(items)) => {
                Self::Selected(string_set(Some(&Value::Array(items.clone()))))
            }
            _ => Self::None,
        }
    }

    fn clears(&self, tool_name: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Selected(names) => names.contains(tool_name),
        }
    }
}

fn string_set(value: Option<&Value>) -> BTreeSet<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn tool_result_contents(messages: &[Value]) -> BTreeMap<String, Value> {
    let mut results = BTreeMap::new();
    for block in content_blocks(messages) {
        if block.get("type").and_then(Value::as_str) != Some("tool_result") {
            continue;
        }
        if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
            results.insert(
                id.to_string(),
                block.get("content").cloned().unwrap_or(Value::Null),
            );
        }
    }
    results
}

fn tool_uses_in_order(messages: &[Value]) -> Vec<(String, String)> {
    content_blocks(messages)
        .filter_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("tool_use")).then(|| {
                (
                    block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
        })
        .filter(|(id, _)| !id.is_empty())
        .collect()
}

fn content_blocks(messages: &[Value]) -> impl Iterator<Item = &Value> {
    messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
}

fn content_blocks_mut(messages: &mut [Value]) -> impl Iterator<Item = &mut Value> {
    messages
        .iter_mut()
        .filter_map(|message| message.get_mut("content").and_then(Value::as_array_mut))
        .flatten()
}

fn last_tool_result_id(messages: &[Value]) -> Option<String> {
    content_blocks(messages)
        .filter_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("tool_result"))
                .then(|| block.get("tool_use_id").and_then(Value::as_str))
                .flatten()
                .map(str::to_string)
        })
        .last()
}

fn is_tiny_result(content: &Value) -> bool {
    text_size(content).is_some_and(|size| size > 0 && size <= TINY_RESULT_BYTES)
}

fn text_size(content: &Value) -> Option<usize> {
    match content {
        Value::String(text) => Some(text.len()),
        Value::Array(parts) => {
            let mut total = 0usize;
            for part in parts {
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    return None;
                }
                total += part.get("text").and_then(Value::as_str)?.len();
            }
            Some(total)
        }
        _ => None,
    }
}

fn select_clear_batch(
    candidates: &[(String, String)],
    results: &BTreeMap<String, Value>,
    messages: &[Value],
    input_tools: &ClearToolInputs,
    clear_at_least: i64,
) -> Vec<String> {
    if clear_at_least <= 0 {
        return candidates.iter().map(|(id, _)| id.clone()).collect();
    }
    let inputs = tool_inputs_by_id(messages);
    let mut selected_end = 0usize;
    let mut batch_tokens = 0i64;
    for (index, (id, name)) in candidates.iter().enumerate() {
        let result = results.get(id).cloned().unwrap_or(Value::Null);
        let input = inputs.get(id).cloned().unwrap_or(Value::Null);
        let clears_input = input_tools.clears(name);
        let before = json!([
            {"type": "tool_result", "content": result},
            {"type": "tool_use", "input": input}
        ]);
        let after = if clears_input {
            json!([
                {
                    "type": "tool_result",
                    "content": CLEARED_TOOL_RESULT_PLACEHOLDER
                },
                {"type": "tool_use", "input": {}}
            ])
        } else {
            json!([
                {
                    "type": "tool_result",
                    "content": CLEARED_TOOL_RESULT_PLACEHOLDER
                },
                {"type": "tool_use", "input": input}
            ])
        };
        batch_tokens += (estimate_value_tokens(&before) - estimate_value_tokens(&after)).max(0);
        if batch_tokens >= clear_at_least {
            selected_end = index + 1;
            batch_tokens = 0;
        }
    }
    candidates[..selected_end]
        .iter()
        .map(|(id, _)| id.clone())
        .collect()
}

fn tool_inputs_by_id(messages: &[Value]) -> BTreeMap<String, Value> {
    content_blocks(messages)
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| {
            Some((
                block.get("id").and_then(Value::as_str)?.to_string(),
                block.get("input").cloned().unwrap_or(Value::Null),
            ))
        })
        .collect()
}

fn clear_selected(
    messages: &mut [Value],
    selected: &[String],
    input_tools: &ClearToolInputs,
) -> u64 {
    let selected = selected.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut cleared = 0u64;
    for block in content_blocks_mut(messages) {
        let kind = block.get("type").and_then(Value::as_str);
        if kind == Some("tool_result")
            && block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .is_some_and(|id| selected.contains(id))
        {
            let replacement = match block.get("content") {
                Some(Value::Array(_)) => {
                    json!([{"type": "text", "text": CLEARED_TOOL_RESULT_PLACEHOLDER}])
                }
                _ => Value::String(CLEARED_TOOL_RESULT_PLACEHOLDER.to_string()),
            };
            if let Some(object) = block.as_object_mut() {
                object.insert("content".to_string(), replacement);
                cleared += 1;
            }
        } else if kind == Some("tool_use")
            && block
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| selected.contains(id))
            && block
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| input_tools.clears(name))
        {
            if let Some(object) = block.as_object_mut() {
                object.insert("input".to_string(), Value::Object(Map::new()));
            }
        }
    }
    cleared
}

fn estimate_value_tokens(value: &Value) -> i64 {
    let bytes = serde_json::to_vec(value)
        .map(|body| body.len())
        .unwrap_or_default();
    ((bytes as u64).saturating_add(3) / 4) as i64
}

fn render_transcript(body: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(system) = body.get("system") {
        if let Some(text) = system_text(system) {
            lines.push(json!({"system": clip(&text, TRANSCRIPT_LINE_CHARS)}).to_string());
        }
    }
    if let Some(messages) = body.get("messages").and_then(Value::as_array) {
        for message in messages {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user");
            match message.get("content") {
                Some(Value::String(text)) => {
                    lines.push(json!({role: clip(text, TRANSCRIPT_LINE_CHARS)}).to_string());
                }
                Some(Value::Array(blocks)) => {
                    for block in blocks {
                        if let Some(line) = transcript_block(role, block) {
                            lines.push(line);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    clip_transcript(lines.join("\n"))
}

fn transcript_block(role: &str, block: &Value) -> Option<String> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => Some(
            json!({
                role: clip(
                    block.get("text").and_then(Value::as_str).unwrap_or_default(),
                    TRANSCRIPT_LINE_CHARS
                )
            })
            .to_string(),
        ),
        "tool_use" => Some(
            json!({
                "assistant_tool_call": {
                    "name": block.get("name"),
                    "input": block.get("input")
                }
            })
            .to_string(),
        ),
        "tool_result" => Some(
            json!({
                "tool_result": clip(
                    &value_text(block.get("content").unwrap_or(&Value::Null)),
                    4_000
                )
            })
            .to_string(),
        ),
        "compaction" => Some(
            json!({
                "conversation_summary": clip(
                    block.get("content").and_then(Value::as_str).unwrap_or_default(),
                    TRANSCRIPT_LINE_CHARS
                )
            })
            .to_string(),
        ),
        _ => None,
    }
}

fn system_text(system: &Value) -> Option<String> {
    match system {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

fn clip(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut clipped = text.chars().take(limit).collect::<String>();
    clipped.push_str(" [truncated]");
    clipped
}

fn clip_transcript(transcript: String) -> String {
    let char_count = transcript.chars().count();
    if char_count <= TRANSCRIPT_HEAD_CHARS + TRANSCRIPT_TAIL_CHARS {
        return transcript;
    }
    let head = transcript
        .chars()
        .take(TRANSCRIPT_HEAD_CHARS)
        .collect::<String>();
    let skip = char_count - TRANSCRIPT_TAIL_CHARS;
    let tail = transcript.chars().skip(skip).collect::<String>();
    format!("{head}\n{{\"meta\":\"middle turns omitted during compaction\"}}\n{tail}")
}

pub(crate) fn apply_summary_to_responses_body(body: &[u8], summary: &str) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };
    let Some(object) = value.as_object_mut() else {
        return body.to_vec();
    };
    let previous_input = object
        .get("input")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let preserved_developer_messages = previous_input
        .iter()
        .filter(|item| item.get("role").and_then(Value::as_str) == Some("developer"))
        .cloned()
        .collect::<Vec<_>>();
    let latest_user = previous_input
        .iter()
        .rev()
        .find(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        .cloned()
        .unwrap_or_else(|| {
            json!({
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "Please continue based on the conversation summary above."
                }]
            })
        });
    let mut input = preserved_developer_messages;
    input.push(json!({
        "type": "message",
        "role": "developer",
        "content": [{
            "type": "input_text",
            "text": format!("Previous conversation summary:\n{summary}")
        }]
    }));
    input.push(latest_user);
    object.insert("input".to_string(), Value::Array(input));
    serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
}

pub(crate) fn summary_prompt(request: &CompactRequest) -> String {
    format!(
        "{}\n\nConversation transcript, one JSON object per item:\n{}",
        request.instructions, request.transcript
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_history(count: usize) -> Value {
        let mut messages = vec![json!({"role": "user", "content": "fix it"})];
        for index in 0..count {
            messages.push(json!({
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": format!("tool_{index}"),
                    "name": "Read",
                    "input": {"path": format!("file-{index}.rs")}
                }]
            }));
            messages.push(json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": format!("tool_{index}"),
                    "content": format!("result-{index}-{}", "x".repeat(200))
                }]
            }));
        }
        json!({
            "model": "gpt-test",
            "messages": messages,
            "context_management": {
                "edits": [{
                    "type": CLEAR_TOOL_USES_EDIT_TYPE,
                    "trigger": {"type": "tool_uses", "value": 1},
                    "keep": {"type": "tool_uses", "value": 2}
                }]
            }
        })
    }

    #[test]
    fn clears_old_tool_results_and_keeps_latest_completed_result() {
        let mut body = tool_history(5);
        let state = prepare_context_management(&mut body).expect("prepare context management");
        let enrichment = state.response_enrichment().expect("applied edit");
        assert_eq!(
            enrichment.applied_edits[0]["cleared_tool_uses"],
            Value::from(3)
        );
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(
            messages[2]["content"][0]["content"],
            CLEARED_TOOL_RESULT_PLACEHOLDER
        );
        assert_eq!(
            messages[4]["content"][0]["content"],
            CLEARED_TOOL_RESULT_PLACEHOLDER
        );
        assert_eq!(
            messages[6]["content"][0]["content"],
            CLEARED_TOOL_RESULT_PLACEHOLDER
        );
        assert_ne!(
            messages[10]["content"][0]["content"],
            CLEARED_TOOL_RESULT_PLACEHOLDER
        );
        assert!(body.get("context_management").is_none());
    }

    #[test]
    fn compact_validates_minimum_trigger_and_builds_plan() {
        let mut body = json!({
            "model": "gpt-test",
            "messages": [{"role": "user", "content": "x".repeat(400_000)}],
            "context_management": {"edits": [{
                "type": COMPACT_EDIT_TYPE,
                "trigger": {"type": "input_tokens", "value": 50_000}
            }]}
        });
        let state = prepare_context_management(&mut body).expect("compact plan");
        assert!(state.needs_compaction_summary());
        assert!(state
            .compact_request()
            .is_some_and(|request| request.transcript.contains("xxxx")));

        let mut invalid = json!({
            "messages": [],
            "context_management": {"edits": [{
                "type": COMPACT_EDIT_TYPE,
                "trigger": {"type": "input_tokens", "value": 10}
            }]}
        });
        assert!(prepare_context_management(&mut invalid)
            .expect_err("minimum trigger")
            .contains("at least 50000"));
    }

    #[test]
    fn compact_summary_replaces_history_and_injects_response_block() {
        let mut state = ContextManagementState::default();
        state.requested = true;
        state.compact_edit = Some(json!({"type": COMPACT_EDIT_TYPE}));
        state.record_compaction(CompactionResult {
            summary: "state and next steps".to_string(),
            input_tokens: 120,
            output_tokens: 20,
        });
        let enrichment = state.response_enrichment().expect("enrichment");
        assert_eq!(
            enrichment.compaction_value().expect("compaction")["content"],
            "state and next steps"
        );
        assert_eq!(
            enrichment.compaction_usage_value().expect("usage")["type"],
            "compaction"
        );

        let adapted = json!({
            "model": "gpt-test",
            "input": [
                {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "system instructions"}]},
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "old"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "old answer"}]},
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "latest"}]}
            ]
        });
        let rewritten = apply_summary_to_responses_body(
            serde_json::to_vec(&adapted).expect("body").as_slice(),
            "state and next steps",
        );
        let rewritten: Value = serde_json::from_slice(&rewritten).expect("rewritten");
        let input = rewritten["input"].as_array().expect("input");
        assert_eq!(input.len(), 3);
        assert_eq!(input[0]["content"][0]["text"], "system instructions");
        assert!(input[1]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("state and next steps")));
        assert_eq!(input[2]["content"][0]["text"], "latest");
    }

    #[test]
    fn maps_openai_compaction_threshold_to_anthropic_trigger() {
        let mut body = json!({
            "model": "gpt-test",
            "messages": [{"role": "user", "content": "x".repeat(220_000)}],
            "context_management": [{
                "type": "compaction",
                "compact_threshold": 50_000,
                "instructions": "keep command outputs"
            }]
        });
        let state = prepare_context_management(&mut body).expect("mapped compact");
        let request = state.compact_request().expect("compact request");
        assert_eq!(request.trigger_tokens, 50_000);
        assert!(request.current_tokens > request.trigger_tokens);
        assert_eq!(request.instructions, "keep command outputs");
    }
}
