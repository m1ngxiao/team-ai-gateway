//! Real loopback sockets distinguish client cancellation from an upstream failure.
//! All accounts, keys and responses are synthetic; no external service is used.
#![cfg(unix)]
#![allow(dead_code)]
#[path = "gateway_logs/support.rs"]
mod support;
use support::*;
use codexmanager_core::storage::{ConversationBinding, RequestLog, UsageSnapshotRecord};
use std::io;
use std::net::Shutdown;
use std::os::fd::AsRawFd;

const MODEL: &str = "gpt-5.3-codex";
const FIRST_SECRET: &str = "synthetic-disconnect-first-key";
const SECOND_SECRET: &str = "synthetic-disconnect-second-key";
const MARKER: &str = "synthetic-visible-before-disconnect";

fn seed(storage: &Storage, tag: &str) {
    seed_model_catalog_models(storage, &[MODEL]);
    let now = now_ts();
    // Two eligible accounts are essential: legacy routing deliberately ignores
    // cooldown for the final candidate, hiding this regression in a one-account pool.
    for index in 0..2 {
        let id = format!("synthetic-{tag}-{index}");
        storage.insert_account(&Account {
            id: id.clone(), label: id.clone(), issuer: "https://invalid.example".into(),
            chatgpt_account_id: Some(format!("fake-{id}")), workspace_id: None,
            group_name: Some(format!("disconnect-{tag}")), sort: index,
            status: "active".into(), created_at: now + index, updated_at: now + index,
        }).unwrap();
        storage.insert_token(&Token {
            account_id: id.clone(), id_token: String::new(), access_token: format!("fake-{id}"),
            refresh_token: String::new(), api_key_access_token: None, last_refresh: now,
        }).unwrap();
        storage.insert_usage_snapshot(&UsageSnapshotRecord {
            account_id: id, used_percent: Some(10.0), window_minutes: Some(300),
            resets_at: None, secondary_used_percent: None, secondary_window_minutes: None,
            secondary_resets_at: None, credits_json: None, captured_at: now,
        }).unwrap();
    }
    for (suffix, secret) in [("first", FIRST_SECRET), ("second", SECOND_SECRET)] {
        let id = format!("key-{tag}-{suffix}");
        let hash = hash_platform_key_for_test(secret);
        storage.insert_api_key(&ApiKey {
            id: id.clone(), name: Some(id.clone()), model_slug: Some(MODEL.into()),
            upstream_provider: Default::default(),
            reasoning_effort: None, service_tier: None, rotation_strategy: "account_rotation".into(),
            aggregate_api_id: None, account_plan_filter: None, aggregate_api_url: None,
            client_type: "codex".into(), protocol_type: "openai_compat".into(),
            auth_scheme: "authorization_bearer".into(), upstream_base_url: None,
            static_headers_json: None, key_hash: hash.clone(), status: "active".into(),
            created_at: now, last_used_at: None,
        }).unwrap();
        storage.update_api_key_account_group_filter(&id, Some(&format!("disconnect-{tag}"))).unwrap();
        let cache = format!("cache-{tag}-{suffix}");
        let digest = Sha256::digest(format!("cache-affinity:v2\0{hash}\0openai_compat\0{MODEL}\0pck\0{cache}").as_bytes());
        let route_id = format!("pck:v2:{}", digest[..16].iter().map(|b| format!("{b:02x}")).collect::<String>());
        storage.upsert_conversation_binding(&ConversationBinding {
            platform_key_hash: hash, conversation_id: route_id.clone(),
            account_id: format!("synthetic-{tag}-0"), thread_epoch: 1,
            thread_anchor: route_id, status: "active".into(), last_model: Some(MODEL.into()),
            last_switch_reason: None, created_at: now, updated_at: now, last_used_at: now,
        }).unwrap();
    }
}

fn event(value: serde_json::Value) -> String { format!("data: {value}\n\n") }
fn created() -> String {
    event(serde_json::json!({"type":"response.created","response":{"id":"resp_disconnect","object":"response","model":MODEL,"status":"in_progress"}}))
}
fn partial(tool: bool) -> String {
    let value = if tool {
        serde_json::json!({"type":"response.output_item.added","output_index":0,"item":{"id":"fc_disconnect","type":"function_call","call_id":MARKER,"name":"get_answer","arguments":"","status":"in_progress"}})
    } else {
        serde_json::json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"msg_disconnect","delta":MARKER})
    };
    created() + event(value).as_str()
}
fn completed() -> String {
    let item = serde_json::json!({"id":"msg_complete","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"synthetic followup ok","annotations":[]}]});
    let mut body = created();
    body += &event(serde_json::json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"msg_complete","delta":"synthetic followup ok"}));
    body += &event(serde_json::json!({"type":"response.completed","response":{"id":"resp_complete","model":MODEL,"status":"completed","output":[item],"usage":{"input_tokens":3,"output_tokens":3,"total_tokens":6}}}));
    body
}
fn request(tag: &str, suffix: &str, tool: bool) -> String {
    let mut body = serde_json::json!({"model":MODEL,"input":"synthetic complete context","stream":true,"prompt_cache_key":format!("cache-{tag}-{suffix}")});
    if tool {
        body["tools"] = serde_json::json!([{"type":"function","name":"get_answer","parameters":{"type":"object","properties":{"q":{"type":"string"}}}}]);
    }
    body.to_string()
}
fn chunk(stream: &mut TcpStream, body: &str) -> io::Result<()> {
    write!(stream, "{:x}\r\n{}\r\n", body.len(), body)?;
    stream.flush()
}
fn accept_until(listener: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "synthetic upstream did not receive expected request");
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("synthetic accept: {error}"),
        }
    }
}
fn upstream_auth(stream: &mut TcpStream) -> String {
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buf = [0; 2048];
        let count = stream.read(&mut buf).unwrap();
        assert_ne!(count, 0, "synthetic upstream request closed");
        bytes.extend_from_slice(&buf[..count]);
        if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") { break offset + 4; }
        assert!(bytes.len() < 65536);
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).to_string();
    let length = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
    }).unwrap_or(0);
    while bytes.len() < header_end + length {
        let mut buf = [0; 2048];
        let count = stream.read(&mut buf).unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&buf[..count]);
    }
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization").then(|| value.trim().to_string())
    }).expect("synthetic upstream authorization")
}
fn start_upstream(tool: bool, truncate_upstream: bool) -> (String, mpsc::Sender<()>, Receiver<bool>, Receiver<String>, thread::JoinHandle<()>) {
    let listener = bind_test_listener("disconnect synthetic upstream");
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (release_tx, release_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = mpsc::channel();
    let (auth_tx, auth_rx) = mpsc::channel();
    let join = thread::spawn(move || {
        let mut first = accept_until(&listener);
        auth_tx.send(upstream_auth(&mut first)).unwrap();
        first.set_write_timeout(Some(Duration::from_secs(1))).unwrap();
        first.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
        chunk(&mut first, &partial(tool)).unwrap();
        // The client has actually observed output before either fault is injected.
        release_rx.recv_timeout(Duration::from_secs(8)).expect("client observed partial output");
        if truncate_upstream {
            // No response.completed and no final HTTP chunk: an actual upstream read failure.
            first.shutdown(Shutdown::Both).unwrap();
            drop(first);
            closed_tx.send(true).unwrap();
        } else {
            first.set_read_timeout(Some(Duration::from_millis(30))).unwrap();
            let continuation = event(serde_json::json!({"type":if tool {"response.function_call_arguments.delta"} else {"response.output_text.delta"},"output_index":0,"content_index":0,"item_id":if tool {"fc_disconnect"} else {"msg_disconnect"},"delta":"x".repeat(16384)}));
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut saw_close = false;
            while Instant::now() < deadline {
                // A post-RST write forces delivery failure even if the first flush succeeded.
                if chunk(&mut first, &continuation).is_err() { saw_close = true; break; }
                match first.read(&mut [0u8; 1]) {
                    Ok(0) => { saw_close = true; break; }
                    Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
                    Err(_) => { saw_close = true; break; }
                    Ok(_) => {}
                }
            }
            closed_tx.send(saw_close).unwrap();
            drop(first);
        }
        let mut second = accept_until(&listener);
        auth_tx.send(upstream_auth(&mut second)).unwrap();
        let body = completed();
        write!(second, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        second.flush().unwrap();
    });
    (address, release_tx, closed_rx, auth_rx, join)
}
fn open_stream(addr: &str, body: &str) -> TcpStream {
    let mut client = TcpStream::connect(addr).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    write!(client, "POST /v1/responses HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {FIRST_SECRET}\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut observed = Vec::new();
    while !String::from_utf8_lossy(&observed).contains(MARKER) {
        let mut buf = [0; 4096];
        let count = client.read(&mut buf).expect("client receives partial stream");
        assert_ne!(count, 0, "stream ended before visible output: {}", String::from_utf8_lossy(&observed));
        observed.extend_from_slice(&buf[..count]);
        assert!(observed.len() < 65536);
    }
    let observed = String::from_utf8_lossy(&observed);
    assert!(observed.starts_with("HTTP/1.1 200") || observed.starts_with("HTTP/1.0 200"));
    client
}
fn reset_client(client: TcpStream) {
    let linger = libc::linger { l_onoff: 1, l_linger: 0 };
    // SAFETY: valid socket fd and initialized linger with its exact C size.
    let result = unsafe {
        libc::setsockopt(client.as_raw_fd(), libc::SOL_SOCKET, libc::SO_LINGER,
            &linger as *const _ as *const libc::c_void,
            std::mem::size_of_val(&linger) as libc::socklen_t)
    };
    assert_eq!(result, 0, "set synthetic client RST: {}", io::Error::last_os_error());
    drop(client);
}
fn metrics(addr: &str) -> HashMap<String, u64> {
    let mut connection = TcpStream::connect(addr).unwrap();
    connection.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    write!(connection, "GET /metrics HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").unwrap();
    let mut body = String::new();
    connection.read_to_string(&mut body).unwrap();
    body.lines().filter_map(|line| {
        let (name, value) = line.split_once(' ')?;
        Some((name.to_string(), value.trim().parse().ok()?))
    }).collect()
}
fn finalized(storage: &Storage, addr: &str, key_id: &str) -> (RequestLog, HashMap<String, u64>) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let current = metrics(addr);
        let logs = storage.list_request_logs(Some(&format!("key:={key_id}")), 10).unwrap();
        if current.get("codexmanager_gateway_requests_active") == Some(&0)
            && current.get("codexmanager_gateway_account_inflight_total") == Some(&0) {
            if let Some(log) = logs.into_iter().find(|log| log.key_id.as_deref() == Some(key_id)) {
                return (log, current);
            }
        }
        assert!(Instant::now() < deadline, "request was not finalized with guards released");
        thread::sleep(Duration::from_millis(5));
    }
}
fn run_case(tag: &str, tool: bool, truncate_upstream: bool) {
    let _lock = test_env_guard();
    let _dynamic = EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED", "true");
    let _cap = EnvGuard::set("CODEXMANAGER_ACCOUNT_MAX_INFLIGHT", "1");
    let _workers = EnvGuard::set("CODEXMANAGER_HTTP_STREAM_WORKER_MIN", "4");
    let _workers_max = EnvGuard::set("CODEXMANAGER_HTTP_STREAM_WORKER_MAX", "4");
    let dir = new_test_dir(tag);
    let db = dir.join("codexmanager.db");
    let _db = EnvGuard::set("CODEXMANAGER_DB_PATH", db.to_str().unwrap());
    let storage = Storage::open(&db).unwrap();
    storage.init().unwrap();
    seed(&storage, tag);
    let (address, release, closed, auth, upstream) = start_upstream(tool, truncate_upstream);
    let _upstream = EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL", &format!("http://{address}/backend-api/codex"));
    let _proxy = EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL", "");
    let server = TestServer::start();
    let before = metrics(&server.addr)["codexmanager_gateway_cooldown_marks_total"];
    let mut client = open_stream(&server.addr, &request(tag, "first", tool));
    assert_eq!(auth.recv_timeout(Duration::from_secs(2)).unwrap(), format!("Bearer fake-synthetic-{tag}-0"));
    if truncate_upstream {
        release.send(()).unwrap();
        let mut remainder = Vec::new();
        let _ = client.read_to_end(&mut remainder);
        drop(client);
    } else {
        reset_client(client);
        release.send(()).unwrap();
    }
    assert!(closed.recv_timeout(Duration::from_secs(10)).unwrap(), "gateway must cancel upstream after the client resets");
    let (first_log, after) = finalized(&storage, &server.addr, &format!("key-{tag}-first"));
    let (status, body) = post_http_raw_with_read_timeout(&server.addr, "/v1/responses", &request(tag, "second", false),
        &[("Content-Type", "application/json"), ("Accept", "text/event-stream"),
          ("Authorization", &format!("Bearer {SECOND_SECRET}"))], Duration::from_secs(10));
    let followup_auth = auth.recv_timeout(Duration::from_secs(2)).unwrap();
    upstream.join().unwrap();
    let (second_log, _) = finalized(&storage, &server.addr, &format!("key-{tag}-second"));
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("response.completed") && body.contains("synthetic followup ok"), "{body}");
    assert_eq!(second_log.status_code, Some(200));
    if truncate_upstream {
        assert_eq!(first_log.status_code, Some(502), "actual upstream truncation must remain a failure");
        assert!(after["codexmanager_gateway_cooldown_marks_total"] > before, "actual upstream failures must still cool accounts");
        assert_eq!(followup_auth, format!("Bearer fake-synthetic-{tag}-1"), "another key must avoid the failed upstream account");
    } else {
        assert_eq!(first_log.status_code, Some(499), "client cancellation must remain classified as 499");
        assert_eq!(after["codexmanager_gateway_cooldown_marks_total"], before, "client delivery failure must not cool an otherwise healthy account");
        assert_eq!(followup_auth, format!("Bearer fake-synthetic-{tag}-0"), "another key bound to the same account must remain unaffected");
    }
}

#[test]
fn client_reset_after_text_does_not_cool_account_for_another_key() {
    run_case("disconnect-text", false, false);
}
#[test]
fn client_reset_after_tool_output_does_not_cool_account_for_another_key() {
    run_case("disconnect-tool", true, false);
}
#[test]
fn upstream_truncation_still_cools_account_for_another_key() {
    run_case("upstream-truncation", false, true);
}
