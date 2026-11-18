use super::*;

#[test]
fn artifact_mcp_publish_and_private_view_work_over_http() {
    let _lock = test_env_guard();
    let dir = new_test_dir("codexmanager-gateway-artifact-mcp");
    let db_path = dir.join("codexmanager.db");
    let artifact_dir = dir.join("artifacts");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db_path.to_string_lossy().as_ref());
    let _artifact_guard = EnvGuard::set(
        "CODEXMANAGER_ARTIFACTS_DIR",
        artifact_dir.to_string_lossy().as_ref(),
    );
    let _public_guard = EnvGuard::set("CODEXMANAGER_ARTIFACTS_PUBLIC_BASE_URL", "");

    let storage = Storage::open(&db_path).expect("open db");
    storage.init().expect("init db");
    let now = now_ts();
    let platform_key = "pk-artifact-mcp-http";
    storage
        .insert_api_key(&ApiKey {
            id: "gk-artifact-mcp-http".to_string(),
            name: Some("artifact-mcp-http".to_string()),
            model_slug: None,
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
            key_hash: hash_platform_key_for_test(platform_key),
            status: "active".to_string(),
            created_at: now,
            last_used_at: None,
        })
        .expect("insert api key");

    let server = codexmanager_service::start_one_shot_server().expect("start tools/list server");
    let (status, body) = post_http_raw(
        &server.addr,
        "/v1/code/mcp/ccr-artifacts",
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        &[
            ("Content-Type", "application/json"),
            ("x-api-key", platform_key),
        ],
    );
    server.join();
    assert_eq!(status, 200, "tools/list response: {body}");
    let tools: serde_json::Value = serde_json::from_str(&body).expect("tools/list json");
    assert!(tools["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .any(|tool| tool["name"] == "Artifact"));

    let server = codexmanager_service::start_one_shot_server().expect("start publish server");
    let publish = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "Artifact",
            "arguments": {
                "action": "publish",
                "content": "<!doctype html><title>MCP Review</title><h1>works</h1>",
                "favicon": "📄"
            }
        }
    });
    let (status, body) = post_http_raw(
        &server.addr,
        "/v1/code/mcp/ccr-artifacts",
        &serde_json::to_string(&publish).expect("publish json"),
        &[
            ("Content-Type", "application/json"),
            ("x-api-key", platform_key),
        ],
    );
    server.join();
    assert_eq!(status, 200, "publish response: {body}");
    let published: serde_json::Value = serde_json::from_str(&body).expect("publish json");
    let artifact = &published["result"]["structuredContent"];
    let slug = artifact["slug"].as_str().expect("artifact slug");
    let view_path = artifact["path"].as_str().expect("artifact path");
    assert_eq!(artifact["title"], "MCP Review");

    let server = codexmanager_service::start_one_shot_server().expect("start metadata server");
    let http = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("http client");
    let metadata = http
        .get(format!("http://{}/v1/artifacts/{slug}", server.addr))
        .header("authorization", format!("Bearer {platform_key}"))
        .send()
        .expect("metadata request");
    assert_eq!(metadata.status(), reqwest::StatusCode::OK);
    let metadata: serde_json::Value = metadata.json().expect("metadata json");
    server.join();
    assert_eq!(metadata["slug"], slug);
    assert_eq!(metadata["title"], "MCP Review");

    let server = codexmanager_service::start_one_shot_server().expect("start view server");
    let view = http
        .get(format!("http://{}{view_path}", server.addr))
        .send()
        .expect("artifact view request");
    assert_eq!(view.status(), reqwest::StatusCode::OK);
    let view = view.text().expect("artifact html");
    server.join();
    assert!(view.contains("<h1>works</h1>"));
}
