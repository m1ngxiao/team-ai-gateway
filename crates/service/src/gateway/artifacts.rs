// Artifact action and response conventions follow wave-agent (MIT), commit
// 7f762789195feeb986a40ebd23f7d75279824913, and Claude Code's public tool shape.
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tiny_http::{Header, Method, Request, Response};

const DEFAULT_FAVICON: &str = "📄";
const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
const ENV_ARTIFACTS_DIR: &str = "CODEXMANAGER_ARTIFACTS_DIR";
const ENV_PUBLIC_BASE_URL: &str = "CODEXMANAGER_ARTIFACTS_PUBLIC_BASE_URL";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtifactRecord {
    slug: String,
    title: String,
    favicon: String,
    owner_key_hash: String,
    version: u64,
    created_at: u64,
    updated_at: u64,
    view_token: String,
}

pub(crate) fn handle_request(request: Request) -> Result<Option<Request>, String> {
    let path = request
        .url()
        .split('?')
        .next()
        .unwrap_or(request.url())
        .to_string();
    if path == "/v1/code/mcp/ccr-artifacts" {
        handle_mcp(request)?;
        return Ok(None);
    }
    if path == "/v1/artifacts" {
        handle_artifacts_collection(request)?;
        return Ok(None);
    }
    if let Some(rest) = path
        .strip_prefix("/code/artifact/")
        .or_else(|| path.strip_prefix("/artifact/"))
    {
        if !valid_slug(rest) {
            return respond_json_error(request, 404, "artifact not found").map(|_| None);
        }
        serve_artifact_view(request, rest)?;
        return Ok(None);
    }
    if let Some(rest) = path.strip_prefix("/v1/artifacts/") {
        handle_artifact_path(request, rest.to_string())?;
        return Ok(None);
    }
    Ok(Some(request))
}

fn handle_artifacts_collection(mut request: Request) -> Result<(), String> {
    let (storage, _, api_key) =
        match super::local_validation::authenticate_platform_request(&request, false) {
            Ok(authenticated) => authenticated,
            Err(err) => {
                return respond_json_error(request, err.status_code, err.message.as_str());
            }
        };
    drop(storage);
    match request.method() {
        Method::Post => {
            let body = read_body(&mut request)?;
            let payload: Value = serde_json::from_slice(&body)
                .map_err(|err| format!("invalid artifact request JSON: {err}"))?;
            let record = publish_artifact(
                &api_key.key_hash,
                payload
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "content is required".to_string())?,
                payload.get("title").and_then(Value::as_str),
                payload.get("favicon").and_then(Value::as_str),
                payload.get("slug").and_then(Value::as_str),
            )?;
            let response = json!({
                "slug": record.slug,
                "url": artifact_url(&request, &record),
                "path": artifact_view_path(&record),
                "title": record.title,
                "favicon": record.favicon,
                "version": record.version
            });
            respond_json(request, 201, response)
        }
        Method::Get => {
            let records = list_artifacts(&api_key.key_hash)?;
            let artifacts = records
                .into_iter()
                .map(|record| {
                    json!({
                        "slug": record.slug,
                        "url": artifact_url(&request, &record),
                        "path": artifact_view_path(&record),
                        "title": record.title,
                        "favicon": record.favicon,
                        "version": record.version,
                        "updatedAt": record.updated_at
                    })
                })
                .collect::<Vec<_>>();
            respond_json(request, 200, json!({"artifacts": artifacts}))
        }
        _ => respond_json_error(request, 405, "method not allowed"),
    }
}

fn handle_artifact_path(request: Request, rest: String) -> Result<(), String> {
    let (slug, action) = match rest.split_once('/') {
        Some((slug, action)) => (slug, Some(action)),
        None => (rest.as_str(), None),
    };
    if !valid_slug(slug) {
        return respond_json_error(request, 404, "artifact not found");
    }
    match action {
        Some("view") => serve_artifact_view(request, slug),
        Some("content") => {
            let (storage, _, api_key) =
                match super::local_validation::authenticate_platform_request(&request, false) {
                    Ok(authenticated) => authenticated,
                    Err(err) => {
                        return respond_json_error(request, err.status_code, err.message.as_str());
                    }
                };
            drop(storage);
            let record = load_record(slug)?;
            if record.owner_key_hash != api_key.key_hash {
                return respond_json_error(request, 404, "artifact not found");
            }
            serve_artifact_content(request, &record)
        }
        Some(_) => respond_json_error(request, 404, "artifact not found"),
        None => match request.method() {
            Method::Get => {
                let (storage, _, api_key) =
                    match super::local_validation::authenticate_platform_request(&request, false) {
                        Ok(authenticated) => authenticated,
                        Err(err) => {
                            return respond_json_error(
                                request,
                                err.status_code,
                                err.message.as_str(),
                            );
                        }
                    };
                drop(storage);
                let record = load_record(slug)?;
                if record.owner_key_hash != api_key.key_hash {
                    return respond_json_error(request, 404, "artifact not found");
                }
                let response = json!({
                    "slug": record.slug,
                    "url": artifact_url(&request, &record),
                    "path": artifact_view_path(&record),
                    "title": record.title,
                    "favicon": record.favicon,
                    "version": record.version,
                    "updatedAt": record.updated_at
                });
                respond_json(request, 200, response)
            }
            Method::Delete => {
                let (storage, _, api_key) =
                    match super::local_validation::authenticate_platform_request(&request, false) {
                        Ok(authenticated) => authenticated,
                        Err(err) => {
                            return respond_json_error(
                                request,
                                err.status_code,
                                err.message.as_str(),
                            );
                        }
                    };
                drop(storage);
                let record = load_record(slug)?;
                if record.owner_key_hash != api_key.key_hash {
                    return respond_json_error(request, 404, "artifact not found");
                }
                let directory = artifact_dir(slug)?;
                if directory.exists() {
                    fs::remove_dir_all(&directory)
                        .map_err(|err| format!("delete artifact failed: {err}"))?;
                }
                respond_json(request, 200, json!({"deleted": true, "slug": slug}))
            }
            _ => respond_json_error(request, 405, "method not allowed"),
        },
    }
}

fn serve_artifact_view(request: Request, slug: &str) -> Result<(), String> {
    let record = load_record(slug)?;
    let token = query_value(request.url(), "token").unwrap_or_default();
    if token != record.view_token {
        return respond_json_error(request, 404, "artifact not found");
    }
    let content = load_content(slug)?;
    let mut response = Response::from_data(content);
    for (name, value) in [
        ("Content-Type", "text/html; charset=utf-8"),
        ("X-Content-Type-Options", "nosniff"),
        ("Cache-Control", "private, max-age=0, must-revalidate"),
        (
            "Content-Security-Policy",
            "default-src 'none'; img-src data: https:; style-src 'unsafe-inline' https:; script-src 'unsafe-inline' https:; font-src data: https:; connect-src https:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
    ] {
        response.add_header(header(name, value));
    }
    request
        .respond(response)
        .map_err(|err| format!("respond artifact view failed: {err}"))
}

fn serve_artifact_content(request: Request, record: &ArtifactRecord) -> Result<(), String> {
    let content = load_content(&record.slug)?;
    let mut response = Response::from_data(content);
    response.add_header(header("Content-Type", "text/html; charset=utf-8"));
    response.add_header(header("X-Content-Type-Options", "nosniff"));
    request
        .respond(response)
        .map_err(|err| format!("respond artifact content failed: {err}"))
}

fn handle_mcp(mut request: Request) -> Result<(), String> {
    if request.method() != &Method::Post {
        return respond_json_error(request, 405, "MCP endpoint accepts POST");
    }
    let (storage, _, api_key) =
        match super::local_validation::authenticate_platform_request(&request, false) {
            Ok(authenticated) => authenticated,
            Err(err) => {
                return respond_json_error(request, err.status_code, err.message.as_str());
            }
        };
    drop(storage);
    let body = read_body(&mut request)?;
    let payload: Value = serde_json::from_slice(&body)
        .map_err(|err| format!("invalid MCP JSON-RPC request: {err}"))?;
    let id = payload.get("id").cloned().unwrap_or(Value::Null);
    let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
    if id.is_null() {
        let _ = request.respond(Response::empty(202));
        return Ok(());
    }
    let result = match method {
        "initialize" => {
            let protocol_version = payload
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-03-26");
            Ok(json!({
                "protocolVersion": protocol_version,
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "team-ai-gateway-artifacts",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Use these tools to publish, read, list or delete private team artifacts. Published links contain an unguessable view token."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": mcp_tools()})),
        "tools/call" => {
            let params = payload.get("params").cloned().unwrap_or_else(|| json!({}));
            Ok(match call_mcp_tool(&request, &api_key.key_hash, params) {
                Ok(result) => result,
                Err(message) => json!({
                    "content": [{"type": "text", "text": message}],
                    "isError": true
                }),
            })
        }
        _ => Err(format!("unknown MCP method: {method}")),
    };
    let response = match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(message) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32602, "message": message}
        }),
    };
    respond_json(request, 200, response)
}

fn call_mcp_tool(request: &Request, owner_key_hash: &str, params: Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tool name is required".to_string())?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let action = match name {
        "Artifact" | "artifact" => args
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("publish"),
        "publish_artifact" => "publish",
        "read_artifact" => "read",
        "list_artifacts" => "list",
        "delete_artifact" => "delete",
        _ => return Err(format!("unknown tool: {name}")),
    };
    let result = execute_artifact_action(request, owner_key_hash, action, &args)?;
    Ok(json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string())
        }],
        "structuredContent": result,
        "isError": false
    }))
}

fn execute_artifact_action(
    request: &Request,
    owner_key_hash: &str,
    action: &str,
    args: &Value,
) -> Result<Value, String> {
    match action.trim().to_ascii_lowercase().as_str() {
        "publish" | "update" => {
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    if args.get("file_path").is_some() {
                        "file_path is a client-local path; this remote MCP server cannot read it. Write the HTML/Markdown to disk, read it with the client's Read tool, and pass its complete contents as content."
                            .to_string()
                    } else {
                        "content is required".to_string()
                    }
                })?;
            let requested_slug = args
                .get("slug")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    args.get("url")
                        .and_then(Value::as_str)
                        .and_then(extract_slug)
                        .filter(|_| args.get("content").is_some())
                });
            let record = publish_artifact(
                owner_key_hash,
                content,
                args.get("title").and_then(Value::as_str),
                args.get("favicon").and_then(Value::as_str),
                requested_slug.as_deref(),
            )?;
            Ok(json!({
                "slug": record.slug,
                "url": artifact_url(request, &record),
                "path": artifact_view_path(&record),
                "title": record.title,
                "favicon": record.favicon,
                "version": record.version
            }))
        }
        "read" => {
            let slug = required_artifact_slug(args)?;
            let record = load_record(slug.as_str())?;
            if record.owner_key_hash != owner_key_hash {
                return Err("artifact not found".to_string());
            }
            let content = String::from_utf8(load_content(slug.as_str())?)
                .map_err(|_| "artifact is not valid UTF-8".to_string())?;
            Ok(json!({
                "slug": record.slug,
                "url": artifact_url(request, &record),
                "path": artifact_view_path(&record),
                "title": record.title,
                "version": record.version,
                "content": content
            }))
        }
        "list" => Ok(json!({
            "artifacts": list_artifacts(owner_key_hash)?
                .into_iter()
                .map(|record| json!({
                    "slug": record.slug,
                    "url": artifact_url(request, &record),
                    "path": artifact_view_path(&record),
                    "title": record.title,
                    "favicon": record.favicon,
                    "version": record.version,
                    "updatedAt": record.updated_at
                }))
                .collect::<Vec<_>>()
        })),
        "delete" => {
            let slug = required_artifact_slug(args)?;
            let record = load_record(slug.as_str())?;
            if record.owner_key_hash != owner_key_hash {
                return Err("artifact not found".to_string());
            }
            fs::remove_dir_all(artifact_dir(slug.as_str())?)
                .map_err(|err| format!("delete artifact failed: {err}"))?;
            Ok(json!({"deleted": true, "slug": slug}))
        }
        other => Err(format!("unknown Artifact action: {other}")),
    }
}

fn required_artifact_slug(args: &Value) -> Result<String, String> {
    args.get("slug")
        .or_else(|| args.get("url"))
        .and_then(Value::as_str)
        .and_then(extract_slug)
        .ok_or_else(|| "slug or artifact url is required".to_string())
}

fn mcp_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "Artifact",
            "description": "Publish, read, list or delete a private shareable web page. Use action=publish (the default) with complete HTML in content. This remote server cannot read a client-local file_path; read the file with the client Read tool first. Chinese requests such as 发布网页, 做成页面, 分享链接, 查看页面, 创作集 map to this tool.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["publish", "read", "list", "delete"],
                        "description": "Operation to perform; defaults to publish."
                    },
                    "content": {
                        "type": "string",
                        "description": "Complete HTML document. Required for publish/update."
                    },
                    "file_path": {
                        "type": "string",
                        "description": "Client-local file path accepted by the Claude Code tool contract. The remote MCP server cannot read it; copy its text into content."
                    },
                    "title": {"type": "string", "description": "Optional page title."},
                    "favicon": {"type": "string", "description": "Optional 1-2 emoji favicon."},
                    "slug": {"type": "string", "description": "Existing artifact slug to update/read/delete."},
                    "url": {"type": "string", "description": "Artifact URL; an alternative to slug."},
                    "prompt": {"type": "string", "description": "Optional focus for a future summarizer; ignored for owned pages."},
                    "scope": {"type": "string", "enum": ["mine", "shared", "all"], "description": "List scope; this server only has pages owned by the current key."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 200}
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "publish_artifact",
            "description": "Publish HTML as a private, shareable artifact and return its URL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": {"type": "string", "description": "Complete HTML document content."},
                    "title": {"type": "string", "description": "Optional page title."},
                    "favicon": {"type": "string", "description": "Optional 1-2 emoji."},
                    "slug": {"type": "string", "description": "Optional existing artifact slug to update."}
                },
                "required": ["content"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "read_artifact",
            "description": "Read an artifact owned by this platform key.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": {"type": "string"},
                    "url": {"type": "string"}
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_artifacts",
            "description": "List artifacts owned by this platform key.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
        json!({
            "name": "delete_artifact",
            "description": "Permanently delete an artifact owned by this platform key.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "slug": {"type": "string"},
                    "url": {"type": "string"}
                },
                "additionalProperties": false
            }
        }),
    ]
}

fn publish_artifact(
    owner_key_hash: &str,
    content: &str,
    title: Option<&str>,
    favicon: Option<&str>,
    requested_slug: Option<&str>,
) -> Result<ArtifactRecord, String> {
    if content.len() > MAX_ARTIFACT_BYTES {
        return Err("artifact content exceeds 16 MiB".to_string());
    }
    if content.trim().is_empty() {
        return Err("artifact content is empty".to_string());
    }
    let slug = requested_slug
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(random_hex_16);
    if !valid_slug(slug.as_str()) {
        return Err("invalid artifact slug".to_string());
    }
    let now = now_ts();
    let directory = artifact_dir(slug.as_str())?;
    fs::create_dir_all(&directory)
        .map_err(|err| format!("create artifact directory failed: {err}"))?;
    let existing = load_record(slug.as_str()).ok();
    if let Some(existing) = existing.as_ref() {
        if existing.owner_key_hash != owner_key_hash {
            return Err("artifact not found".to_string());
        }
    }
    let title = normalize_title(title, content);
    let favicon = normalize_favicon(favicon);
    let view_token = existing
        .as_ref()
        .map(|record| record.view_token.clone())
        .unwrap_or_else(random_hex_16);
    let record = ArtifactRecord {
        slug,
        title,
        favicon,
        owner_key_hash: owner_key_hash.to_string(),
        version: existing
            .as_ref()
            .map(|record| record.version + 1)
            .unwrap_or(1),
        created_at: existing
            .as_ref()
            .map(|record| record.created_at)
            .unwrap_or(now),
        updated_at: now,
        view_token,
    };
    atomic_write(directory.join("index.html"), content.as_bytes())?;
    atomic_write(
        directory.join("artifact.json"),
        serde_json::to_vec_pretty(&record)
            .map_err(|err| format!("serialize artifact metadata failed: {err}"))?
            .as_slice(),
    )?;
    Ok(record)
}

fn list_artifacts(owner_key_hash: &str) -> Result<Vec<ArtifactRecord>, String> {
    let root = artifacts_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(&root).map_err(|err| format!("list artifacts failed: {err}"))? {
        let entry = entry.map_err(|err| format!("read artifact entry failed: {err}"))?;
        if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            continue;
        }
        let slug = entry.file_name().to_string_lossy().to_string();
        if !valid_slug(slug.as_str()) {
            continue;
        }
        if let Ok(record) = load_record(slug.as_str()) {
            if record.owner_key_hash == owner_key_hash {
                records.push(record);
            }
        }
    }
    records.sort_by_key(|record| std::cmp::Reverse(record.updated_at));
    Ok(records)
}

fn load_record(slug: &str) -> Result<ArtifactRecord, String> {
    if !valid_slug(slug) {
        return Err("artifact not found".to_string());
    }
    let path = artifact_dir(slug)?.join("artifact.json");
    let bytes = fs::read(&path).map_err(|_| "artifact not found".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "artifact not found".to_string())
}

fn load_content(slug: &str) -> Result<Vec<u8>, String> {
    if !valid_slug(slug) {
        return Err("artifact not found".to_string());
    }
    fs::read(artifact_dir(slug)?.join("index.html")).map_err(|_| "artifact not found".to_string())
}

fn artifacts_root() -> PathBuf {
    if let Some(path) = std::env::var_os(ENV_ARTIFACTS_DIR) {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    if let Some(db_path) = std::env::var_os("CODEXMANAGER_DB_PATH") {
        if let Some(parent) = PathBuf::from(db_path).parent() {
            return parent.join("artifacts");
        }
    }
    PathBuf::from("data").join("artifacts")
}

fn artifact_dir(slug: &str) -> Result<PathBuf, String> {
    if !valid_slug(slug) {
        return Err("artifact not found".to_string());
    }
    Ok(artifacts_root().join(slug))
}

fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn extract_slug(value: &str) -> Option<String> {
    let value = value.trim();
    if valid_slug(value) {
        return Some(value.to_string());
    }
    let path = value.split('?').next().unwrap_or(value);
    for marker in ["/v1/artifacts/", "/code/artifact/", "/artifact/"] {
        if let Some(rest) = path.split_once(marker).map(|(_, rest)| rest) {
            let slug = rest.split('/').next().unwrap_or_default().trim();
            if valid_slug(slug) {
                return Some(slug.to_string());
            }
        }
    }
    None
}

fn normalize_title(title: Option<&str>, content: &str) -> String {
    title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(200).collect())
        .or_else(|| extract_html_title(content))
        .unwrap_or_else(|| "Untitled artifact".to_string())
}

fn extract_html_title(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let end = lower[open_end..].find("</title>")? + open_end;
    let title = content[open_end..end].trim();
    (!title.is_empty()).then(|| title.chars().take(200).collect())
}

fn normalize_favicon(favicon: Option<&str>) -> String {
    let value = favicon.map(str::trim).unwrap_or_default();
    if value.is_empty()
        || value.len() > 16
        || value.contains('<')
        || value.contains('>')
        || value.contains("://")
    {
        return DEFAULT_FAVICON.to_string();
    }
    value.to_string()
}

fn artifact_url(request: &Request, record: &ArtifactRecord) -> String {
    let base = std::env::var(ENV_PUBLIC_BASE_URL)
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| request_origin(request))
        .unwrap_or_else(|| "http://127.0.0.1:48760".to_string());
    format!(
        "{base}/v1/artifacts/{}/view?token={}",
        urlencoding::encode(&record.slug),
        record.view_token
    )
}

fn artifact_view_path(record: &ArtifactRecord) -> String {
    format!(
        "/v1/artifacts/{}/view?token={}",
        record.slug, record.view_token
    )
}

fn request_origin(request: &Request) -> Option<String> {
    let host = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("host"))
        .map(|header| header.value.as_str().trim().to_string())
        .filter(|value| !value.is_empty())?;
    let scheme = request
        .headers()
        .iter()
        .find(|header| header.field.equiv("x-forwarded-proto"))
        .map(|header| header.value.as_str().trim())
        .filter(|value| *value == "http" || *value == "https")
        .unwrap_or("http");
    Some(format!("{scheme}://{host}"))
}

fn query_value(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for item in query.split('&') {
        let (candidate, value) = item.split_once('=').unwrap_or((item, ""));
        if candidate == key {
            return urlencoding::decode(value)
                .ok()
                .map(|value| value.into_owned());
        }
    }
    None
}

fn read_body(request: &mut Request) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    request
        .as_reader()
        .read_to_end(&mut body)
        .map_err(|err| format!("read request body failed: {err}"))?;
    Ok(body)
}

fn respond_json(request: Request, status: u16, value: Value) -> Result<(), String> {
    let body = serde_json::to_vec(&value)
        .map_err(|err| format!("serialize artifact response failed: {err}"))?;
    let mut response = Response::from_data(body).with_status_code(status);
    response.add_header(header("Content-Type", "application/json; charset=utf-8"));
    request
        .respond(response)
        .map_err(|err| format!("respond artifact request failed: {err}"))
}

fn respond_json_error(request: Request, status: u16, message: &str) -> Result<(), String> {
    respond_json(
        request,
        status,
        json!({"error": {"type": "artifact_error", "message": message}}),
    )
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("static artifact response header must be valid")
}

fn atomic_write(path: PathBuf, data: &[u8]) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| "artifact output directory is missing".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|err| format!("create artifact output directory failed: {err}"))?;
    let temp = directory.join(format!(".tmp-{}", random_hex_16()));
    {
        let mut file =
            fs::File::create(&temp).map_err(|err| format!("create artifact file failed: {err}"))?;
        file.write_all(data)
            .map_err(|err| format!("write artifact file failed: {err}"))?;
        file.sync_all()
            .map_err(|err| format!("sync artifact file failed: {err}"))?;
    }
    fs::rename(&temp, &path).map_err(|err| {
        let _ = fs::remove_file(&temp);
        format!("replace artifact file failed: {err}")
    })
}

fn random_hex_16() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn validates_slugs_and_extracts_urls() {
        assert!(valid_slug("abc_123"));
        assert!(!valid_slug("../secret"));
        assert_eq!(
            extract_slug("https://example.test/v1/artifacts/abc_123/view?token=x").as_deref(),
            Some("abc_123")
        );
    }

    #[test]
    fn extracts_html_title_and_defaults_favicon() {
        assert_eq!(
            normalize_title(None, "<html><title>Deploy Review</title></html>"),
            "Deploy Review"
        );
        assert_eq!(normalize_favicon(Some("https://bad")), DEFAULT_FAVICON);
    }

    #[test]
    fn mcp_exposes_claude_code_style_artifact_action_tool() {
        let tools = mcp_tools();
        let artifact = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some("Artifact"))
            .expect("Artifact tool");
        let actions = artifact
            .pointer("/inputSchema/properties/action/enum")
            .and_then(Value::as_array)
            .expect("action enum");
        assert_eq!(actions.len(), 4);
        assert!(actions.iter().any(|action| action == "publish"));
        assert!(artifact
            .pointer("/inputSchema/properties/content")
            .is_some());
        assert!(artifact
            .pointer("/inputSchema/properties/file_path")
            .is_some());
    }

    #[test]
    fn publishes_updates_and_lists_owned_artifacts() {
        let _guard = TEST_ENV_LOCK.lock().expect("artifact test env lock");
        let root =
            std::env::temp_dir().join(format!("team-gateway-artifact-test-{}", random_hex_16()));
        let previous = std::env::var_os(ENV_ARTIFACTS_DIR);
        std::env::set_var(ENV_ARTIFACTS_DIR, &root);

        let first = publish_artifact(
            "owner-a",
            "<!doctype html><title>First</title>",
            None,
            Some("📄"),
            None,
        )
        .expect("publish artifact");
        assert_eq!(first.version, 1);
        assert!(artifact_dir(&first.slug)
            .expect("artifact directory")
            .join("index.html")
            .is_file());

        let updated = publish_artifact(
            "owner-a",
            "<!doctype html><title>Second</title>",
            None,
            None,
            Some(&first.slug),
        )
        .expect("update artifact");
        assert_eq!(updated.version, 2);
        assert_eq!(list_artifacts("owner-a").expect("list artifacts").len(), 1);
        assert!(list_artifacts("owner-b")
            .expect("list other owner")
            .is_empty());

        if let Some(previous) = previous {
            std::env::set_var(ENV_ARTIFACTS_DIR, previous);
        } else {
            std::env::remove_var(ENV_ARTIFACTS_DIR);
        }
        let _ = fs::remove_dir_all(root);
    }
}
