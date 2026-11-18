// Manual client-level compatibility test. Run after installing Claude Code:
// CLAUDE_CODE_E2E_BIN=/path/to/claude \
//   cargo test -p codexmanager-service --test gateway_logs \
//   claude_code_e2e::claude_code_2_1_312_uses_candidate_gateway \
//   --offline --locked -- --ignored --exact --nocapture
use super::*;
use std::io::Write;
use std::process::Command;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

const MODEL: &str = "gpt-5.4-mini";
const KEY: &str = "cc-e2e-synthetic-platform-key";

fn response_event(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

fn text_response(text: &str) -> String {
    let mut output = response_event(serde_json::json!({
        "type": "response.created",
        "response": {
            "id": "resp_cc_e2e",
            "model": MODEL,
            "status": "in_progress"
        }
    }));
    output.push_str(&response_event(serde_json::json!({
        "type": "response.output_text.delta",
        "delta": text
    })));
    output.push_str(&response_event(serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": "resp_cc_e2e",
            "model": MODEL,
            "status": "completed",
            "output": [],
            "usage": {
                "input_tokens": 12,
                "output_tokens": 4,
                "total_tokens": 16
            }
        }
    })));
    output
}

fn function_call_response(tool_name: &str, arguments: serde_json::Value) -> String {
    let item = serde_json::json!({
        "type": "function_call",
        "call_id": "toolu_cc_e2e",
        "name": tool_name,
        "arguments": serde_json::to_string(&arguments).expect("tool arguments"),
        "status": "completed"
    });
    let mut output = response_event(serde_json::json!({
        "type": "response.created",
        "response": {
            "id": "resp_cc_tool",
            "model": MODEL,
            "status": "in_progress"
        }
    }));
    output.push_str(&response_event(serde_json::json!({
        "type": "response.output_item.done",
        "item": item
    })));
    output.push_str(&response_event(serde_json::json!({
        "type": "response.completed",
        "response": {
            "id": "resp_cc_tool",
            "model": MODEL,
            "status": "completed",
            "output": [item],
            "usage": {
                "input_tokens": 20,
                "output_tokens": 8,
                "total_tokens": 28
            }
        }
    })));
    output
}

fn mock_response(payload: &serde_json::Value) -> String {
    let serialized = serde_json::to_string(payload).unwrap_or_default();
    if payload
        .get("instructions")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|text| text.contains("safety classifier"))
    {
        return text_response(
            r#"{"verdicts":{"toolu_cc_e2e":{"flagged":false,"explanation":"routine test command"}}}"#,
        );
    }
    if payload
        .get("instructions")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|text| text.starts_with("Compact the supplied"))
    {
        return text_response("<summary>CC_COMPACTION_SUMMARY: parser state retained</summary>");
    }
    if serialized.contains("REMINDER: Do NOT call any tools")
        || serialized.contains("<analysis> block followed by a <summary> block")
    {
        return text_response(
            "<analysis>Claude Code requested a client-side compaction summary.</analysis>\n<summary>CC_COMPACT_SUMMARY: parser state retained</summary>",
        );
    }
    if serialized.contains("\"type\":\"function_call_output\"") {
        if serialized.to_ascii_lowercase().contains("artifact") {
            return text_response("CC_ARTIFACT_DONE");
        }
        return text_response("CC_TOOL_DONE");
    }
    let tools = payload
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if serialized.contains("publish an Artifact") {
        if let Some(name) = tools.iter().find_map(|tool| {
            let name = tool.get("name").and_then(serde_json::Value::as_str)?;
            name.to_ascii_lowercase()
                .contains("artifact")
                .then_some(name)
        }) {
            return function_call_response(
                name,
                serde_json::json!({
                    "action": "publish",
                    "content": "<!doctype html><title>CC E2E Artifact</title><h1>claude code works</h1>",
                    "favicon": "📄"
                }),
            );
        }
    }
    if serialized.contains("run cc-tool through Bash") {
        if tools
            .iter()
            .any(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some("Bash"))
        {
            return function_call_response(
                "Bash",
                serde_json::json!({"command": "printf CC_TOOL_OK"}),
            );
        }
    }
    if serialized.contains("CC_LONG_CONTEXT") {
        return text_response("CC_LONG_OK");
    }
    text_response("CC_NORMAL_OK")
}

fn start_mock_upstream() -> (
    String,
    Arc<Mutex<Vec<serde_json::Value>>>,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("mock upstream server");
    let addr = server
        .server_addr()
        .to_ip()
        .expect("mock upstream addr")
        .to_string();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_for_thread = Arc::clone(&captured);
    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    let join = std::thread::spawn(move || loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }
        let Some(request) = server
            .recv_timeout(Duration::from_millis(100))
            .ok()
            .flatten()
        else {
            continue;
        };
        let mut request = request;
        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);
        let payload = serde_json::from_str::<serde_json::Value>(&body)
            .unwrap_or_else(|_| serde_json::json!({"_raw": body}));
        let sse = mock_response(&payload);
        captured_for_thread
            .lock()
            .expect("captured requests")
            .push(payload);
        let response = tiny_http::Response::from_string(sse).with_status_code(200);
        let mut response = response;
        response.add_header(
            tiny_http::Header::from_bytes("Content-Type", "text/event-stream")
                .expect("content type"),
        );
        let _ = request.respond(response);
    });
    (addr, captured, stop_tx, join)
}

fn wait_for_gateway(addr: &str) {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .expect("health client");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        if client
            .get(format!("http://{addr}/health"))
            .send()
            .is_ok_and(|response| response.status().is_success())
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "gateway health timeout"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn claude_command(
    claude_bin: &str,
    home: &std::path::Path,
    config_dir: &std::path::Path,
    workdir: &std::path::Path,
    gateway_addr: &str,
) -> Command {
    let mut command = Command::new("timeout");
    command
        .arg("120s")
        .arg(claude_bin)
        .current_dir(workdir)
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .env("ANTHROPIC_BASE_URL", format!("http://{gateway_addr}"))
        .env("ANTHROPIC_API_KEY", KEY)
        .env("ANTHROPIC_AUTH_TOKEN", KEY)
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("CLAUDE_CODE_DISABLE_UPDATE_CHECK", "1");
    command
}

fn assert_claude_success(output: &std::process::Output, expected: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "Claude Code failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(expected),
        "Claude Code output did not contain {expected}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

fn run_interactive_compact(
    claude_bin: &str,
    home: &std::path::Path,
    config_dir: &std::path::Path,
    workdir: &std::path::Path,
    gateway_addr: &str,
    prompt: String,
) -> std::process::Output {
    let command = format!(
        "{} --bare --model {} --tools '' --autocompact 1M --permission-mode dontAsk",
        claude_bin, MODEL
    );
    let mut child = Command::new("timeout")
        .arg("90s")
        .arg("script")
        .arg("-qec")
        .arg(command)
        .arg("/dev/null")
        .current_dir(workdir)
        .env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .env("ANTHROPIC_BASE_URL", format!("http://{gateway_addr}"))
        .env("ANTHROPIC_API_KEY", KEY)
        .env("ANTHROPIC_AUTH_TOKEN", KEY)
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .env("DISABLE_TELEMETRY", "1")
        .env("CLAUDE_CODE_DISABLE_UPDATE_CHECK", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn interactive Claude Code");
    let mut stdin = child.stdin.take().expect("interactive stdin");
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let _ = stdin.write_all(prompt.as_bytes());
        std::thread::sleep(Duration::from_millis(300));
        let _ = stdin.write_all(b"\r");
        std::thread::sleep(Duration::from_secs(10));
        let _ = stdin.write_all(b"/compact\r");
        std::thread::sleep(Duration::from_secs(20));
        let _ = stdin.write_all(b"/exit\r");
    });
    child
        .wait_with_output()
        .expect("wait for interactive Claude Code")
}

#[test]
#[ignore = "requires CLAUDE_CODE_E2E_BIN and an installed Claude Code release"]
fn claude_code_2_1_312_uses_candidate_gateway() {
    let claude_bin = std::env::var("CLAUDE_CODE_E2E_BIN")
        .expect("set CLAUDE_CODE_E2E_BIN to the Claude Code executable");
    let _lock = test_env_guard();
    let dir = new_test_dir("codexmanager-cc-e2e");
    let db_path = dir.join("codexmanager.db");
    let graph_dir = dir.join("graph");
    std::fs::create_dir_all(&graph_dir).expect("create graph dir");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());
    let _artifacts_guard = EnvGuard::set(
        "CODEXMANAGER_ARTIFACTS_DIR",
        graph_dir.join("artifacts").to_string_lossy().as_ref(),
    );
    let _compression_guard = EnvGuard::set("CODEXMANAGER_ENABLE_REQUEST_COMPRESSION", "false");
    let _proxy_guard = EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL", "");

    let (upstream_addr, captured, stop_upstream, upstream_join) = start_mock_upstream();
    let _upstream_guard = EnvGuard::set(
        "CODEXMANAGER_UPSTREAM_BASE_URL",
        &format!("http://{upstream_addr}/backend-api/codex"),
    );
    let storage = Storage::open(&db_path).expect("open db");
    storage.init().expect("init db");
    seed_model_catalog_models(&storage, &[MODEL]);
    let now = now_ts();
    storage
        .insert_account(&Account {
            id: "acc-cc-e2e".to_string(),
            label: "cc-e2e".to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: Some("chatgpt-cc-e2e".to_string()),
            workspace_id: None,
            group_name: None,
            sort: 0,
            status: "active".to_string(),
            created_at: now,
            updated_at: now,
        })
        .expect("insert account");
    storage
        .insert_token(&Token {
            account_id: "acc-cc-e2e".to_string(),
            id_token: String::new(),
            access_token: "synthetic-access-token".to_string(),
            refresh_token: String::new(),
            api_key_access_token: Some("synthetic-api-key-token".to_string()),
            last_refresh: now,
        })
        .expect("insert token");
    storage
        .insert_api_key(&ApiKey {
            id: "gk-cc-e2e".to_string(),
            name: Some("cc-e2e".to_string()),
            model_slug: Some(MODEL.to_string()),
            reasoning_effort: None,
            service_tier: None,
            upstream_provider: Default::default(),
            rotation_strategy: "account_rotation".to_string(),
            aggregate_api_id: None,
            account_plan_filter: None,
            aggregate_api_url: None,
            client_type: "claude_code".to_string(),
            protocol_type: "anthropic_native".to_string(),
            auth_scheme: "x_api_key".to_string(),
            upstream_base_url: None,
            static_headers_json: None,
            key_hash: hash_platform_key_for_test(KEY),
            status: "active".to_string(),
            created_at: now,
            last_used_at: None,
        })
        .expect("insert api key");

    let listener = bind_test_listener("cc e2e gateway port");
    let gateway_addr = listener.local_addr().expect("gateway addr").to_string();
    drop(listener);
    let gateway_addr_for_thread = gateway_addr.clone();
    std::thread::spawn(move || {
        codexmanager_service::start_server(gateway_addr_for_thread.as_str())
            .expect("gateway server");
    });
    wait_for_gateway(gateway_addr.as_str());

    let home = dir.join("home");
    let config_dir = dir.join("claude-config");
    let workdir = dir.join("workdir");
    std::fs::create_dir_all(&home).expect("home dir");
    std::fs::create_dir_all(&config_dir).expect("claude config dir");
    std::fs::create_dir_all(&workdir).expect("workdir");
    let claude_state_path = config_dir.join(".claude.json");
    let mut claude_state = std::fs::read(&claude_state_path)
        .ok()
        .and_then(|body| serde_json::from_slice::<serde_json::Value>(&body).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    claude_state["hasCompletedOnboarding"] = serde_json::Value::Bool(true);
    claude_state["theme"] = serde_json::Value::String("dark".to_string());
    let key_suffix = KEY
        .chars()
        .rev()
        .take(20)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    claude_state["customApiKeyResponses"] = serde_json::json!({
        "approved": [key_suffix],
        "rejected": []
    });
    let project_key = workdir.to_string_lossy().to_string();
    claude_state["projects"][project_key]["hasTrustDialogAccepted"] = serde_json::Value::Bool(true);
    std::fs::write(
        &claude_state_path,
        serde_json::to_vec(&claude_state).expect("claude state"),
    )
    .expect("write claude state");
    let output = claude_command(
        claude_bin.as_str(),
        home.as_path(),
        config_dir.as_path(),
        workdir.as_path(),
        gateway_addr.as_str(),
    )
    .arg("--bare")
    .arg("--print")
    .arg("--model")
    .arg(MODEL)
    .arg("--tools")
    .arg("")
    .arg("--output-format")
    .arg("json")
    .arg("Reply with exactly CC_NORMAL_OK.")
    .output()
    .expect("run Claude Code");
    assert_claude_success(&output, "CC_NORMAL_OK");

    let long_prompt = format!(
        "{}CC_LONG_CONTEXT: reply with CC_LONG_OK.",
        "context ".repeat(90_000)
    );
    let output = run_interactive_compact(
        claude_bin.as_str(),
        home.as_path(),
        config_dir.as_path(),
        workdir.as_path(),
        gateway_addr.as_str(),
        long_prompt,
    );
    assert!(
        output.status.success(),
        "Interactive Claude Code compact failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("CC_LONG_OK")
            || String::from_utf8_lossy(&output.stderr).contains("CC_LONG_OK"),
        "interactive session did not show the long-context response\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output = claude_command(
        claude_bin.as_str(),
        home.as_path(),
        config_dir.as_path(),
        workdir.as_path(),
        gateway_addr.as_str(),
    )
    .arg("--bare")
    .arg("--print")
    .arg("--model")
    .arg(MODEL)
    .arg("--permission-mode")
    .arg("auto")
    .arg("--tools")
    .arg("Bash")
    .arg("--allowedTools")
    .arg("Bash")
    .arg("--output-format")
    .arg("json")
    .arg("run cc-tool through Bash")
    .output()
    .expect("run Claude Code auto mode");
    assert_claude_success(&output, "CC_TOOL_DONE");

    let mcp_config_path = dir.join("mcp.json");
    std::fs::write(
        &mcp_config_path,
        serde_json::to_vec(&serde_json::json!({
            "mcpServers": {
                "team-artifacts": {
                    "type": "http",
                    "url": format!("http://{gateway_addr}/v1/code/mcp/ccr-artifacts"),
                    "headers": {
                        "Authorization": format!("Bearer {KEY}")
                    }
                }
            }
        }))
        .expect("mcp config"),
    )
    .expect("write mcp config");
    let output = claude_command(
        claude_bin.as_str(),
        home.as_path(),
        config_dir.as_path(),
        workdir.as_path(),
        gateway_addr.as_str(),
    )
    .arg("--bare")
    .arg("--print")
    .arg("--model")
    .arg(MODEL)
    .arg("--mcp-config")
    .arg(&mcp_config_path)
    .arg("--strict-mcp-config")
    .arg("--permission-mode")
    .arg("dontAsk")
    .arg("--allowedTools")
    .arg("mcp__team-artifacts__Artifact")
    .arg("--output-format")
    .arg("json")
    .arg("Use the Artifact tool to publish an Artifact containing claude code works")
    .output()
    .expect("run Claude Code artifact");
    assert_claude_success(&output, "CC_ARTIFACT_DONE");
    let artifact_entries = std::fs::read_dir(graph_dir.join("artifacts"))
        .expect("artifact directory")
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    assert!(
        artifact_entries.iter().any(|entry| {
            std::fs::read_to_string(entry.path().join("index.html"))
                .ok()
                .is_some_and(|html| html.contains("<h1>claude code works</h1>"))
        }),
        "Artifact MCP did not persist the expected page"
    );

    let requests = captured.lock().expect("captured requests").clone();
    assert!(!requests.is_empty(), "upstream received no requests");
    assert_eq!(requests[0]["model"], MODEL);
    assert!(requests[0].get("messages").is_none());
    assert!(requests[0]["input"].is_array());
    assert_eq!(
        requests
            .iter()
            .filter(|request| serde_json::to_string(request)
                .unwrap_or_default()
                .contains("REMINDER: Do NOT call any tools"))
            .count(),
        1,
        "Claude Code compact should issue exactly one successful summary request"
    );
    assert!(requests.iter().any(|request| {
        request
            .get("instructions")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| text.contains("safety classifier"))
    }));

    let _ = stop_upstream.send(());
    upstream_join.join().expect("join mock upstream");
}
