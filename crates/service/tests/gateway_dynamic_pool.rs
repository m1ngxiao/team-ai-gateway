//! Dynamic scheduler enabled: protocol, replay safety, delivery and binding regressions.
//! No production database, tokens, accounts, or external network are used.
#![allow(dead_code)]
#[path = "gateway_logs/support.rs"]
mod support;
use support::*;
use codexmanager_core::storage::UsageSnapshotRecord;

const MODEL: &str = "gpt-5.3-codex";
const KEY: &str = "synthetic-overload-integration-key";
const OVERLOAD: &str = "Our servers are currently overloaded. Please try again later.";

fn seed(storage: &Storage, tag: &str, accounts: usize) {
    seed_model_catalog_models(storage, &[MODEL]);
    let now = now_ts();
    for index in 0..accounts {
        let id = format!("synthetic-{tag}-{index}");
        storage.insert_account(&Account {
            id: id.clone(), label: id.clone(), issuer: "https://invalid.example".into(),
            chatgpt_account_id: Some(format!("fake-{id}")), workspace_id: None,
            group_name: Some(format!("dynamic-{tag}")), sort: index as i64, status: "active".into(),
            created_at: now + index as i64, updated_at: now + index as i64,
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
    storage.insert_api_key(&ApiKey {
        id: format!("key-{tag}"), name: Some(format!("synthetic-{tag}")),
        model_slug: Some(MODEL.into()), reasoning_effort: None, service_tier: None,
        upstream_provider: Default::default(),
        rotation_strategy: "account_rotation".into(), aggregate_api_id: None,
        account_plan_filter: None, aggregate_api_url: None, client_type: "codex".into(),
        protocol_type: "openai_compat".into(), auth_scheme: "authorization_bearer".into(),
        upstream_base_url: None, static_headers_json: None, key_hash: hash_platform_key_for_test(KEY),
        status: "active".into(), created_at: now, last_used_at: None,
    }).unwrap();
    storage.update_api_key_account_group_filter(&format!("key-{tag}"), Some(&format!("dynamic-{tag}"))).unwrap();
}

fn event(value: serde_json::Value) -> String { format!("data: {value}\n\n") }
fn created() -> String {
    event(serde_json::json!({"type":"response.created","response":{"id":"resp_synthetic","object":"response","model":MODEL,"status":"in_progress"}}))
}
fn overload() -> String {
    created() + event(serde_json::json!({"type":"response.failed","response":{
        "id":"resp_failed","status":"failed","error":{"code":"server_is_overloaded","message":OVERLOAD}}})).as_str()
}
fn completed(tool: bool) -> String {
    let item = if tool {
        serde_json::json!({"id":"fc_synthetic","type":"function_call","call_id":"call_synthetic","name":"get_answer","arguments":"{\"q\":\"test\"}","status":"completed"})
    } else {
        serde_json::json!({"id":"msg_synthetic","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"synthetic backup ok","annotations":[]}]})
    };
    let mut sse = created();
    let mut started_item = item.clone();
    started_item["status"] = "in_progress".into();
    if tool { started_item["arguments"] = "".into(); } else { started_item["content"] = serde_json::json!([]); }
    sse += &event(serde_json::json!({"type":"response.output_item.added","output_index":0,"item":started_item}));
    if tool {
        sse += &event(serde_json::json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_synthetic","delta":"{\"q\":\"test\"}"}));
        sse += &event(serde_json::json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":"fc_synthetic","arguments":"{\"q\":\"test\"}"}));
    } else {
        sse += &event(serde_json::json!({"type":"response.content_part.added","output_index":0,"content_index":0,"item_id":"msg_synthetic","part":{"type":"output_text","text":"","annotations":[]}}));
        sse += &event(serde_json::json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"msg_synthetic","delta":"synthetic backup ok"}));
        sse += &event(serde_json::json!({"type":"response.output_text.done","output_index":0,"content_index":0,"item_id":"msg_synthetic","text":"synthetic backup ok"}));
    }
    sse += &event(serde_json::json!({"type":"response.output_item.done","output_index":0,"item":item}));
    sse += &event(serde_json::json!({"type":"response.completed","response":{"id":"resp_synthetic","model":MODEL,"status":"completed","output":[item],"usage":{"input_tokens":3,"output_tokens":3,"total_tokens":6}}}));
    sse
}
fn request(path: &str, stream: bool, tool: bool) -> serde_json::Value {
    let mut value = if path == "/v1/responses" {
        serde_json::json!({"model":MODEL,"input":"synthetic test","stream":stream})
    } else {
        serde_json::json!({"model":MODEL,"messages":[{"role":"user","content":"synthetic test"}],"stream":stream})
    };
    if tool {
        value["tools"] = if path == "/v1/responses" {
            serde_json::json!([{"type":"function","name":"get_answer","description":"Synthetic function","parameters":{"type":"object","properties":{"q":{"type":"string"}}}}])
        } else {
            serde_json::json!([{"type":"function","function":{"name":"get_answer","description":"Synthetic function","parameters":{"type":"object","properties":{"q":{"type":"string"}}}}}])
        };
    }
    value
}
fn sse(body: String) -> (u16, String, String) { (200, body, "text/event-stream".into()) }

struct ResultCase { status: u16, body: String, attempts: Vec<CapturedUpstreamRequest> }
fn run_case(tag: &str, path: &str, body: serde_json::Value, responses: Vec<(u16,String,String)>, account_count: usize) -> ResultCase {
    let _lock = test_env_guard();
    let _dynamic = EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED", "true");
    let dir = new_test_dir(&format!("codexmanager-overload-{tag}"));
    let db = dir.join("codexmanager.db");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH", db.to_str().unwrap());
    let storage = Storage::open(&db).unwrap();
    storage.init().unwrap();
    seed(&storage, tag, account_count);
    let (address, rx, join) = start_mock_upstream_sequence_lenient_with_content_types(responses, Duration::from_secs(3));
    let _upstream = EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL", &format!("http://{address}/backend-api/codex"));
    let _proxy = EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL", "");
    let server = codexmanager_service::start_one_shot_server().unwrap();
    let auth = format!("Bearer {KEY}");
    let (status, response_body) = post_http_raw_with_read_timeout(&server.addr, path, &body.to_string(),
        &[("Content-Type", "application/json"), ("Authorization", &auth), ("Accept", "text/event-stream")], Duration::from_secs(15));
    server.join();
    join.join().unwrap();
    let attempts = rx.try_iter().collect();
    ResultCase {status, body: response_body, attempts}
}
fn assert_two_distinct(result: &ResultCase) {
    assert_eq!(result.attempts.len(), 2, "upstream request count, status={}, body={}", result.status, result.body);
    assert_ne!(result.attempts[0].headers.get("authorization"), result.attempts[1].headers.get("authorization"), "fallback must change synthetic account");
}
fn success_case(tag: &str, path: &str, stream: bool, tool: bool) {
    let result = run_case(tag, path, request(path,stream,tool), vec![sse(overload()),sse(completed(tool))],4);
    assert_eq!(result.status,200,"{}",result.body);
    assert_two_distinct(&result);
    assert!(!result.body.contains(OVERLOAD),"failed first attempt must not reach downstream");
    assert!(result.body.contains(if tool {"get_answer"} else {"synthetic backup ok"}),"{}",result.body);
    if tool {
        let body: serde_json::Value = serde_json::from_slice(&decode_upstream_request_body(&result.attempts[1])).unwrap();
        assert_eq!(body["tools"][0]["name"], "get_answer", "function tools preserved through fallback");
    }
}
#[test] fn overload_responses_stream_recovers() {success_case("responses-stream","/v1/responses",true,false)}
#[test] fn overload_responses_nonstream_recovers() {success_case("responses-json","/v1/responses",false,false)}
#[test] fn overload_chat_stream_recovers() {success_case("chat-stream","/v1/chat/completions",true,false)}
#[test] fn overload_chat_nonstream_recovers() {success_case("chat-json","/v1/chat/completions",false,false)}
#[test] fn overload_responses_stream_preserves_tools() {success_case("responses-stream-tool","/v1/responses",true,true)}
#[test] fn overload_responses_nonstream_preserves_tools() {success_case("responses-json-tool","/v1/responses",false,true)}
#[test] fn overload_chat_stream_preserves_tools() {success_case("chat-stream-tool","/v1/chat/completions",true,true)}
#[test] fn overload_chat_nonstream_preserves_tools() {success_case("chat-json-tool","/v1/chat/completions",false,true)}
#[test] fn all_four_overloaded_stops_after_two_accounts() {
    let result = run_case("all-failed","/v1/responses",request("/v1/responses",true,false),vec![sse(overload()),sse(overload()),sse(overload()),sse(overload())],4);
    assert_two_distinct(&result);
    assert!(result.body.contains("server_is_overloaded") || result.body.contains(OVERLOAD), "overload must remain visible: {}",result.body);
}
fn backup_status(status: u16) {
    let result = run_case(&format!("backup-{status}"),"/v1/responses",request("/v1/responses",true,false),vec![sse(overload()),(status,"{\"error\":{\"message\":\"synthetic temporary failure\"}}".into(),"application/json".into()),sse(completed(false))],4);
    assert_two_distinct(&result);
    assert!(result.status >= 400, "backup failure should be returned: {}",result.body);
    assert!(!result.body.contains("synthetic backup ok"));
}
#[test] fn overload_backup_429_does_not_try_third_account() {backup_status(429)}
#[test] fn overload_backup_503_does_not_try_third_account() {backup_status(503)}
fn committed_prefix_case(tool: bool) {
    let prefix = if tool {
        event(serde_json::json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_first","call_id":"call_first","name":"get_answer","arguments":""}}))
    } else { event(serde_json::json!({"type":"response.output_text.delta","delta":"first-account-visible-text"})) };
    let result = run_case(if tool {"tool-prefix"} else {"text-prefix"},"/v1/responses",request("/v1/responses",true,tool),vec![sse(created()+prefix.as_str()+overload().as_str()),sse(completed(false))],4);
    assert_eq!(result.attempts.len(),1,"no replay after useful output/tool event: {}",result.body);
    assert!(!result.body.contains("synthetic backup ok"));
}
#[test] fn text_before_overload_does_not_replay() {committed_prefix_case(false)}
#[test] fn tool_before_overload_does_not_replay() {committed_prefix_case(true)}
#[test] fn opaque_previous_response_context_does_not_replay() {
    let mut body = request("/v1/responses",true,false);
    body["previous_response_id"] = "resp_opaque_server_side_state".into();
    let result = run_case("opaque-previous","/v1/responses",body,vec![sse(overload()),sse(completed(false))],4);
    assert_eq!(result.attempts.len(),1,"opaque upstream context cannot safely move between accounts");
}
#[test] fn hosted_tool_request_does_not_replay() {
    let mut body = request("/v1/responses",true,false);
    body["tools"] = serde_json::json!([{"type":"web_search_preview"}]);
    let result = run_case("hosted-tool","/v1/responses",body,vec![sse(overload()),sse(completed(false))],4);
    assert_eq!(result.attempts.len(),1,"hosted tools may have server side effects");
}

fn read_synthetic_request(stream: &mut TcpStream) -> String {
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut bytes = Vec::new();
    let end;
    loop {
        let mut chunk = [0u8; 2048];
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0, "synthetic request unexpectedly closed");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(offset) = bytes.windows(4).position(|part| part == b"\r\n\r\n") { end = offset + 4; break; }
        assert!(bytes.len() < 65536);
    }
    let headers = String::from_utf8_lossy(&bytes[..end]).to_string();
    let content_length = headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
    }).unwrap_or(0);
    while bytes.len() < end + content_length {
        let mut chunk = [0u8; 2048];
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case("authorization").then(||value.trim().to_string())
    }).unwrap_or_default()
}
fn write_chunk(stream: &mut TcpStream, value: &str) -> std::io::Result<()> {
    write!(stream, "{:x}\r\n{}\r\n",value.len(),value)?;
    stream.flush()
}
fn insert_another_key(storage: &Storage, source_id: &str, id: &str, secret: &str) {
    let mut key = storage.find_api_key_by_id(source_id).unwrap().unwrap();
    key.id = id.into(); key.name = Some(id.into()); key.key_hash = hash_platform_key_for_test(secret);
    storage.insert_api_key(&key).unwrap();
    let group = storage.find_api_key_account_group_filter(source_id).unwrap();
    storage.update_api_key_account_group_filter(id, group.as_deref()).unwrap();
}
fn post_synthetic(addr: &str, secret: &str) -> (u16, String) {
    post_http_raw_with_read_timeout(addr,"/v1/responses",&request("/v1/responses",true,false).to_string(),
        &[("Content-Type","application/json"),("Accept","text/event-stream"),("Authorization",&format!("Bearer {secret}"))],Duration::from_secs(15))
}

#[test]
fn positive_account_cap_rejects_overbooking_and_recovers_after_completion() {
    use std::sync::{Arc, atomic::AtomicBool};
    let _lock = test_env_guard();
    let _dynamic = EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED", "true");
    let dir = new_test_dir("codexmanager-overload-hardcap");
    let db = dir.join("codexmanager.db");
    let _db_guard = EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
    let _cap = EnvGuard::set("CODEXMANAGER_ACCOUNT_MAX_INFLIGHT","1");
    // At least three stream workers are required to exercise account admission;
    // otherwise the third request waits outside the account-cap code entirely.
    let _stream_min = EnvGuard::set("CODEXMANAGER_HTTP_STREAM_WORKER_MIN","4");
    let _stream_max = EnvGuard::set("CODEXMANAGER_HTTP_STREAM_WORKER_MAX","4");
    let listener = bind_test_listener("hardcap synthetic upstream");
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let _upstream = EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{address}/backend-api/codex"));
    let _proxy = EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
    let storage = Storage::open(&db).unwrap(); storage.init().unwrap(); seed(&storage,"hardcap",2);
    insert_another_key(&storage,"key-hardcap","key-hardcap-second","synthetic-second-key");
    insert_another_key(&storage,"key-hardcap","key-hardcap-third","synthetic-third-key");
    let release = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let (tx,rx) = mpsc::channel();
    let mock_release = Arc::clone(&release); let mock_stop = Arc::clone(&stop);
    let upstream = thread::spawn(move || {
        let mut children = Vec::new();
        let deadline = Instant::now()+Duration::from_secs(20);
        while !mock_stop.load(Ordering::Acquire) && Instant::now()<deadline {
            match listener.accept() {
                Ok((mut stream,_)) => {
                    let tx = tx.clone(); let release = Arc::clone(&mock_release);
                    children.push(thread::spawn(move || {
                        let auth = read_synthetic_request(&mut stream); tx.send(auth).unwrap();
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
                        write_chunk(&mut stream,&(created()+event(serde_json::json!({"type":"response.output_text.delta","delta":"holding synthetic slot"})).as_str())).unwrap();
                        let deadline = Instant::now()+Duration::from_secs(12);
                        while !release.load(Ordering::Acquire) && Instant::now()<deadline {thread::sleep(Duration::from_millis(10));}
                        let _=write_chunk(&mut stream,&completed(false)); let _=stream.write_all(b"0\r\n\r\n");
                    }));
                },
                Err(error) if error.kind()==std::io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(5)),
                Err(error)=> panic!("synthetic upstream accept: {error}"),
            }
        }
        for child in children {child.join().unwrap();}
    });
    let server=TestServer::start();
    let a=server.addr.clone(); let first=thread::spawn(move||post_synthetic(&a,KEY));
    let first_auth=rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let a=server.addr.clone(); let second=thread::spawn(move||post_synthetic(&a,"synthetic-second-key"));
    let second_auth=rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_ne!(first_auth,second_auth,"cap1 must spread the two active requests across two accounts");
    let started=Instant::now();
    let blocked=post_synthetic(&server.addr,"synthetic-third-key");
    assert!(blocked.0>=400,"third request must fail while both accounts are occupied: {blocked:?}");
    assert!(started.elapsed()<Duration::from_secs(3),"no prolonged hard-cap queue");
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(),"last candidate must not bypass cap");
    release.store(true,Ordering::Release);
    assert_eq!(first.join().unwrap().0,200); assert_eq!(second.join().unwrap().0,200);
    let recovered=post_synthetic(&server.addr,"synthetic-third-key");
    assert_eq!(recovered.0,200,"completed requests must release hard-cap guards: {recovered:?}");
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
    stop.store(true,Ordering::Release); upstream.join().unwrap();
    drop(server);
}

#[test]
fn downstream_disconnect_during_visible_stream_cancels_upstream_and_releases_slot() {
    let _lock = test_env_guard();
    let _dynamic = EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED", "true");
    let dir=new_test_dir("codexmanager-overload-downstream-cancel");
    let db=dir.join("codexmanager.db");
    let _db_guard=EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
    let _cap=EnvGuard::set("CODEXMANAGER_ACCOUNT_MAX_INFLIGHT","1");
    let listener=bind_test_listener("cancel synthetic upstream");
    let address=listener.local_addr().unwrap();
    let _upstream=EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{address}/backend-api/codex"));
    let _proxy=EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
    let storage=Storage::open(&db).unwrap(); storage.init().unwrap(); seed(&storage,"cancel",1);
    insert_another_key(&storage,"key-cancel","key-cancel-after","synthetic-after-cancel-key");
    let (tx,rx)=mpsc::channel();
    listener.set_nonblocking(true).unwrap();
    let upstream=thread::spawn(move || {
        let deadline=Instant::now()+Duration::from_secs(20);
        let mut served=0;
        while served<2 && Instant::now()<deadline {
            let mut stream=match listener.accept() {
                Ok((stream,_))=>stream,
                Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=> {thread::sleep(Duration::from_millis(5));continue;},
                Err(error)=>panic!("mock accept: {error}"),
            };
            read_synthetic_request(&mut stream);
            stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            if served==0 {
                let chunk=event(serde_json::json!({"type":"response.output_text.delta","delta":format!("cancel-visible-{}","x".repeat(4096))}));
                let deadline=Instant::now()+Duration::from_secs(8);
                let mut saw_close=false;
                while Instant::now()<deadline {
                    if write_chunk(&mut stream,&chunk).is_err() {saw_close=true;break;}
                    thread::sleep(Duration::from_millis(20));
                }
                let _=tx.send(saw_close);
            } else {
                let _=write_chunk(&mut stream,&completed(false)); let _=stream.write_all(b"0\r\n\r\n");
            }
            served+=1;
        }
        served
    });
    let server=TestServer::start();
    let mut client=TcpStream::connect(&server.addr).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
    let body=request("/v1/responses",true,false).to_string();
    write!(client,"POST /v1/responses HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nAuthorization: Bearer {KEY}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",server.addr,body.len()).unwrap();
    let mut received=Vec::new();
    loop {
        let mut buf=[0u8;8192]; let n=client.read(&mut buf).unwrap(); assert!(n>0);
        received.extend_from_slice(&buf[..n]);
        if String::from_utf8_lossy(&received).contains("cancel-visible-") {break;}
    }
    let cancel_at=Instant::now();
    client.shutdown(std::net::Shutdown::Both).unwrap(); drop(client);
    assert!(rx.recv_timeout(Duration::from_secs(9)).unwrap(),"active upstream must close after downstream fails writes");
    eprintln!("synthetic_visible_stream_cancel_upstream_ms={}",cancel_at.elapsed().as_millis());
    // Transport close can win the race with final logging/guard-drop by a few milliseconds.
    thread::sleep(Duration::from_millis(100));
    let recovered=post_synthetic(&server.addr,"synthetic-after-cancel-key");
    assert_eq!(recovered.0,200,"cancelled stream must release hard-cap guard: {recovered:?}");
    assert_eq!(upstream.join().unwrap(),2);
    drop(server);
}

fn cache_route_id(hash: &str, cache_key: &str) -> String {
    let digest=Sha256::digest(format!("cache-affinity:v2\0{hash}\0openai_compat\0{MODEL}\0pck\0{cache_key}").as_bytes());
    format!("pck:v2:{}",digest[..16].iter().map(|byte|format!("{byte:02x}")).collect::<String>())
}
fn binding_case(backup_success: bool, chat_nonstream: bool) {
    use codexmanager_core::storage::ConversationBinding;
    let _lock=test_env_guard();
    let _dynamic = EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED", "true");
    let tag=if chat_nonstream {"binding-chat-failed"} else if backup_success {"binding-success"} else {"binding-failed"};
    let path=if chat_nonstream {"/v1/chat/completions"} else {"/v1/responses"};
    let dir=new_test_dir(&format!("codexmanager-overload-{tag}"));
    let db=dir.join("codexmanager.db");
    let _db_guard=EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
    let responses=if backup_success {vec![sse(overload()),sse(completed(false)),sse(completed(false))]} else {vec![sse(overload()),sse(overload())]};
    let storage=Storage::open(&db).unwrap(); storage.init().unwrap(); seed(&storage,tag,4);
    let (address,rx,join)=start_mock_upstream_sequence_lenient_with_content_types(responses,Duration::from_secs(3));
    let _upstream=EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{address}/backend-api/codex"));
    let _proxy=EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
    let key_hash=hash_platform_key_for_test(KEY); let cache_key=format!("synthetic-cache-{tag}");
    // Chat normalization does not preserve the Responses-only cache key. Use an
    // explicit native conversation header to exercise an existing Chat binding.
    let route_id=if chat_nonstream {format!("synthetic-conversation-{tag}")} else {cache_route_id(&key_hash,&cache_key)};
    let old=format!("synthetic-{tag}-0");
    let now=now_ts();
    storage.upsert_conversation_binding(&ConversationBinding{
        platform_key_hash:key_hash.clone(),conversation_id:route_id.clone(),account_id:old.clone(),
        thread_epoch:1,thread_anchor:route_id.clone(),status:"active".into(),last_model:Some(MODEL.into()),
        last_switch_reason:None,created_at:now,updated_at:now,last_used_at:now,
    }).unwrap();
    let mut body=request(path,!chat_nonstream,true);
    body["prompt_cache_key"]=cache_key.into();
    if chat_nonstream {
        body["messages"]=serde_json::json!([
            {"role":"user","content":"synthetic earlier question"},
            {"role":"assistant","content":null,"tool_calls":[{"id":"call_history","type":"function","function":{"name":"get_answer","arguments":"{\"q\":\"synthetic\"}"}}]},
            {"role":"tool","tool_call_id":"call_history","content":"synthetic-local-tool-result"},
            {"role":"user","content":"synthetic followup"}
        ]);
    } else {
    body["input"]=serde_json::json!([
        {"role":"user","content":"synthetic earlier question"},
        {"type":"function_call","call_id":"call_history","name":"get_answer","arguments":"{\"q\":\"synthetic\"}"},
        {"type":"function_call_output","call_id":"call_history","output":"synthetic-local-tool-result"},
        {"role":"user","content":"synthetic followup"}
    ]);
    }
    let server=codexmanager_service::start_one_shot_server().unwrap();
    let authorization=format!("Bearer {KEY}");
    let mut headers=vec![("Content-Type","application/json"),("Authorization",authorization.as_str()),("Accept","text/event-stream")];
    if chat_nonstream {headers.push(("conversation_id",route_id.as_str()));}
    let (status,response)=post_http_raw_with_read_timeout(&server.addr,path,&body.to_string(),&headers,Duration::from_secs(10));
    server.join();
    assert_eq!(status,if chat_nonstream && !backup_success {502} else {200},"{response}");
    let first=rx.recv_timeout(Duration::from_secs(2)).unwrap(); let second=rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(first.headers["authorization"].contains(&old));
    assert_ne!(first.headers["authorization"],second.headers["authorization"]);
    let first_body:serde_json::Value=serde_json::from_slice(&decode_upstream_request_body(&first)).unwrap();
    let second_body:serde_json::Value=serde_json::from_slice(&decode_upstream_request_body(&second)).unwrap();
    assert_eq!(first_body["input"],second_body["input"],"fallback cannot drop/rewrite client history or tool output");
    assert!(second_body["input"].to_string().contains("synthetic-local-tool-result"));
    let binding=storage.get_conversation_binding(&key_hash,&route_id).unwrap().unwrap();
    if backup_success {
        assert_ne!(binding.account_id,old,"successful backup must rebind");
        assert!(second.headers["authorization"].contains(&binding.account_id));
        let server=codexmanager_service::start_one_shot_server().unwrap();
        let next=post_http_raw_with_read_timeout(&server.addr,path,&body.to_string(),&[("Content-Type","application/json"),("Authorization",&format!("Bearer {KEY}")),("Accept","text/event-stream")],Duration::from_secs(10));
        server.join(); assert_eq!(next.0,200);
        let third=rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(second.headers["authorization"],third.headers["authorization"],"next request must reuse successful backup");
    } else {
        assert_eq!(binding.account_id,old,"HTTP200 + response.failed must not overwrite an established binding");
        assert_eq!(binding.thread_epoch,1);
    }
    join.join().unwrap(); assert_eq!(rx.try_iter().count(),0);
}
#[test] fn completed_backup_rebinds_and_preserves_history_and_local_tool_output() {binding_case(true,false)}
#[test] fn failed_http200_backup_preserves_old_binding_and_history() {binding_case(false,false)}

/// The mock refuses to finish until the client observes an actual text/tool delta.
/// This detects whole-response buffering without using a performance threshold.
fn chat_stream_handshake_case(tool: bool) {
    use std::sync::{Arc,atomic::AtomicBool};
    let _lock=test_env_guard();
    let _dynamic = EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED", "true");
    let tag=if tool {"chat-tool-handshake"} else {"chat-text-handshake"};
    let dir=new_test_dir(&format!("codexmanager-overload-{tag}"));
    let db=dir.join("codexmanager.db");
    let _db_guard=EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
    let storage=Storage::open(&db).unwrap(); storage.init().unwrap(); seed(&storage,tag,4);
    let listener=bind_test_listener("chat streaming handshake upstream");
    listener.set_nonblocking(true).unwrap();
    let address=listener.local_addr().unwrap();
    let _upstream=EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{address}/backend-api/codex"));
    let _proxy=EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
    let (release_tx,release_rx)=mpsc::channel::<()>();
    let completed_sent=Arc::new(AtomicBool::new(false)); let mock_completed=Arc::clone(&completed_sent);
    let marker=if tool {"handshake-first-arguments"} else {"handshake-visible-text"};
    let mock=thread::spawn(move|| {
        let mut accounts=Vec::new();
        for attempt in 0..2 {
            let deadline=Instant::now()+Duration::from_secs(15);
            let mut stream=loop {
                match listener.accept() {
                    Ok((stream,_))=>break stream,
                    Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=> {
                        assert!(Instant::now()<deadline,"gateway never made expected synthetic attempt {attempt}");
                        thread::sleep(Duration::from_millis(5));
                    },
                    Err(error)=>panic!("synthetic accept: {error}"),
                }
            };
            accounts.push(read_synthetic_request(&mut stream));
            if attempt==0 {
                let failed=overload();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{failed}",failed.len()).unwrap();
                continue;
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            let item=if tool {
                serde_json::json!({"type":"function_call","id":"fc_handshake","call_id":"call_handshake","name":"get_answer","arguments":format!("{{\"q\":\"{marker}\"}}"),"status":"completed"})
            } else {
                serde_json::json!({"type":"message","id":"msg_handshake","role":"assistant","status":"completed","content":[{"type":"output_text","text":marker,"annotations":[]}]})
            };
            let mut started_item=item.clone(); started_item["status"]="in_progress".into();
            if tool {started_item["arguments"]="".into();} else {started_item["content"]=serde_json::json!([]);}
            let mut prefix=created();
            prefix+=&event(serde_json::json!({"type":"response.output_item.added","output_index":0,"item":started_item}));
            if tool {
                prefix+=&event(serde_json::json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_handshake","delta":format!("{{\"q\":\"{marker}\"")}));
            } else {
                prefix+=&event(serde_json::json!({"type":"response.content_part.added","output_index":0,"content_index":0,"item_id":"msg_handshake","part":{"type":"output_text","text":"","annotations":[]}}));
                prefix+=&event(serde_json::json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"item_id":"msg_handshake","delta":marker}));
            }
            write_chunk(&mut stream,&prefix).unwrap();
            // Timeout is only a deadlock guard. Completion is caused by client ACK,
            // never by a fixed sleep or the passage of time.
            release_rx.recv_timeout(Duration::from_secs(12)).expect("client must receive delta before upstream completion");
            let mut suffix=String::new();
            if tool {
                suffix+=&event(serde_json::json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_handshake","delta":"}"}));
                suffix+=&event(serde_json::json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":"fc_handshake","arguments":item["arguments"]}));
            } else {
                suffix+=&event(serde_json::json!({"type":"response.output_text.done","output_index":0,"content_index":0,"item_id":"msg_handshake","text":marker}));
            }
            suffix+=&event(serde_json::json!({"type":"response.output_item.done","output_index":0,"item":item}));
            suffix+=&event(serde_json::json!({"type":"response.completed","response":{"id":"resp_synthetic","model":MODEL,"status":"completed","output":[item],"usage":{"input_tokens":2,"output_tokens":2,"total_tokens":4}}}));
            mock_completed.store(true,Ordering::Release);
            write_chunk(&mut stream,&suffix).unwrap(); stream.write_all(b"0\r\n\r\n").unwrap();
        }
        accounts
    });
    let server=codexmanager_service::start_one_shot_server().unwrap();
    let http=reqwest::blocking::Client::builder().no_proxy().timeout(Duration::from_secs(15)).build().unwrap();
    let mut client=http.post(format!("http://{}/v1/chat/completions",server.addr))
        .bearer_auth(KEY).header("Accept","text/event-stream")
        .json(&request("/v1/chat/completions",true,tool)).send().unwrap();
    assert_eq!(client.status(),reqwest::StatusCode::OK);
    let mut received=Vec::new();
    loop {
        let mut buf=[0u8;8192];
        let count=client.read(&mut buf).expect("first meaningful SSE delta must arrive while upstream completion is blocked");
        assert!(count>0,"response ended before handshake delta"); received.extend_from_slice(&buf[..count]);
        if String::from_utf8_lossy(&received).contains(marker) {break;}
    }
    assert!(!completed_sent.load(Ordering::Acquire),"upstream must still be waiting for the client ACK");
    if tool {
        let prefix=String::from_utf8_lossy(&received);
        assert!(prefix.contains("tool_calls") && prefix.contains("get_answer"),"must deliver a real tool delta: {prefix}");
    }
    release_tx.send(()).unwrap();
    let mut rest=Vec::new(); client.read_to_end(&mut rest).unwrap(); received.extend(rest);
    server.join();
    let accounts=mock.join().unwrap(); assert_eq!(accounts.len(),2); assert_ne!(accounts[0],accounts[1]);
    let raw=String::from_utf8(received).unwrap();
    assert!(raw.contains("[DONE]"),"must preserve client terminal marker");
    let mut arguments=String::new(); let mut text=String::new(); let mut finish_reason=None;
    for line in raw.lines() {
        let Some(data)=line.strip_prefix("data: ") else {continue;};
        let Ok(value)=serde_json::from_str::<serde_json::Value>(data) else {continue;};
        let choice=&value["choices"][0];
        if let Some(reason)=choice["finish_reason"].as_str() {finish_reason=Some(reason.to_string());}
        if let Some(delta)=choice["delta"]["content"].as_str() {text.push_str(delta);}
        if let Some(calls)=choice["delta"]["tool_calls"].as_array() {
            for call in calls {if let Some(delta)=call["function"]["arguments"].as_str() {arguments.push_str(delta);}}
        }
    }
    if tool {
        assert_eq!(finish_reason.as_deref(),Some("tool_calls"));
        assert_eq!(serde_json::from_str::<serde_json::Value>(&arguments).unwrap(),serde_json::json!({"q":marker}),"tool arguments must be forwarded once without duplication");
    } else {
        assert_eq!(finish_reason.as_deref(),Some("stop")); assert_eq!(text,marker);
    }
}
#[test] fn chat_stream_text_arrives_before_upstream_completion_after_failover() {chat_stream_handshake_case(false)}
#[test] fn chat_stream_tool_delta_arrives_before_upstream_completion_after_failover() {chat_stream_handshake_case(true)}

#[test] fn failed_chat_json_backup_preserves_old_binding_and_history() {binding_case(false,true)}
fn chat_all_overloaded_case(stream: bool) {
    let path="/v1/chat/completions";
    let result=run_case(if stream {"chat-all-overloaded-stream"} else {"chat-all-overloaded-json"},path,request(path,stream,false),vec![sse(overload()),sse(overload()),sse(overload()),sse(overload())],4);
    assert_two_distinct(&result);
    assert_eq!(result.status,if stream {200} else {502},"{}",result.body);
    let error=if stream {
        result.body.lines().filter_map(|line|line.strip_prefix("data: "))
            .filter_map(|data|serde_json::from_str::<serde_json::Value>(data).ok())
            .find(|value|value.get("error").is_some()).expect("chat streaming must expose a terminal error SSE")
    } else {serde_json::from_str::<serde_json::Value>(&result.body).expect("chat nonstream error JSON")};
    assert_eq!(error["error"]["code"],"server_is_overloaded");
    assert!(error["error"]["message"].as_str().unwrap_or_default().contains(OVERLOAD));
}
#[test] fn chat_stream_all_overloaded_preserves_explicit_error_after_two_accounts() {chat_all_overloaded_case(true)}
#[test] fn chat_nonstream_all_overloaded_preserves_explicit_error_after_two_accounts() {chat_all_overloaded_case(false)}

// The following cases exercise live scheduler decisions through real HTTP, not
// just the preserved protocol suite above. Channels and observed stream close
// establish ordering; elapsed time is only a failure bound.
struct DynamicMock {
    addr: String,
    arrivals: Receiver<String>,
    seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}
impl DynamicMock {
    fn start(mut respond: impl FnMut(usize, TcpStream) + Send + 'static) -> Self {
        use std::sync::{Arc, Mutex, atomic::AtomicBool};
        let listener=bind_test_listener("dynamic scheduling upstream");
        listener.set_nonblocking(true).unwrap();
        let addr=listener.local_addr().unwrap().to_string();
        let seen=Arc::new(Mutex::new(Vec::new()));
        let stop=Arc::new(AtomicBool::new(false));
        let (tx,arrivals)=mpsc::channel();
        let recorded=Arc::clone(&seen); let stopped=Arc::clone(&stop);
        let join=thread::spawn(move || {
            let deadline=Instant::now()+Duration::from_secs(40);
            while !stopped.load(Ordering::Acquire) && Instant::now()<deadline {
                match listener.accept() {
                    Ok((mut stream,_)) => {
                        let authorization=read_synthetic_request(&mut stream);
                        let ordinal={
                            let mut values=recorded.lock().unwrap();
                            let ordinal=values.len();values.push(authorization.clone());ordinal
                        };
                        tx.send(authorization).unwrap();
                        respond(ordinal,stream);
                    }
                    Err(err) if err.kind()==std::io::ErrorKind::WouldBlock => {
                        thread::park_timeout(Duration::from_millis(2));
                    }
                    Err(err)=>panic!("dynamic mock accept failed: {err}"),
                }
            }
        });
        Self {addr,arrivals,seen,stop,join:Some(join)}
    }
    fn finish(mut self) -> Vec<String> {
        self.stop.store(true,Ordering::Release);
        self.join.take().unwrap().join().unwrap();
        let seen=self.seen.lock().unwrap().clone();
        seen
    }
}
impl Drop for DynamicMock {
    fn drop(&mut self) {
        self.stop.store(true,Ordering::Release);
        if let Some(join)=self.join.take() { let _=join.join(); }
    }
}
fn write_synthetic_sse(stream: &mut TcpStream, value: &str) {
    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{value}",value.len()).unwrap();
    stream.flush().unwrap();
}
fn bind_synthetic_account(storage: &Storage, tag: &str, secret: &str, cache_key: &str, index: usize) {
    let hash=hash_platform_key_for_test(secret);
    let conversation=cache_route_id(&hash,cache_key);
    let now=now_ts();
    storage.upsert_conversation_binding(&codexmanager_core::storage::ConversationBinding {
        platform_key_hash:hash, conversation_id:conversation.clone(),
        account_id:format!("synthetic-{tag}-{index}"), thread_epoch:1,
        thread_anchor:conversation, status:"active".into(), last_model:Some(MODEL.into()),
        last_switch_reason:None, created_at:now, updated_at:now, last_used_at:now,
    }).unwrap();
}
fn body_with_cache(cache_key: &str) -> serde_json::Value {
    let mut body=request("/v1/responses",true,false);
    body["prompt_cache_key"]=cache_key.into();body
}
fn post_synthetic_body(addr: &str, secret: &str, body: serde_json::Value) -> (u16,String) {
    post_http_raw_with_read_timeout(addr,"/v1/responses",&body.to_string(),
        &[("Content-Type","application/json"),("Accept","text/event-stream"),
          ("Authorization",&format!("Bearer {secret}"))],Duration::from_secs(15))
}
fn wait_for_gateway_idle(addr: &str) {
    let deadline=Instant::now()+Duration::from_secs(4);
    loop {
        let mut connection=TcpStream::connect(addr).unwrap();
        connection.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        write!(connection,"GET /metrics HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").unwrap();
        let mut metrics=String::new();connection.read_to_string(&mut metrics).unwrap();
        let zero=|name: &str|metrics.lines().any(|line|line.trim()==format!("{name} 0"));
        if zero("codexmanager_gateway_requests_active") && zero("codexmanager_gateway_account_inflight_total") {return;}
        assert!(Instant::now()<deadline,"gateway did not release completed request guards");
        thread::yield_now();
    }
}

#[test]
fn live_backup_chooses_idle_account_over_next_candidate_busy_on_another_key() {
    let _lock=test_env_guard();
    let _dynamic=EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED","true");
    let _cap=EnvGuard::set("CODEXMANAGER_ACCOUNT_MAX_INFLIGHT","0");
    let _workers=EnvGuard::set("CODEXMANAGER_HTTP_STREAM_WORKER_MIN","4");
    let _workers_max=EnvGuard::set("CODEXMANAGER_HTTP_STREAM_WORKER_MAX","4");
    let tag="live-load-cross-key";
    let dir=new_test_dir(tag);let db=dir.join("codexmanager.db");
    let _db=EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
    let storage=Storage::open(&db).unwrap();storage.init().unwrap();seed(&storage,tag,3);
    let other="synthetic-live-load-other-key";
    insert_another_key(&storage,&format!("key-{tag}"),"key-live-load-other",other);
    for (secret,cache,index) in [(KEY,"load-warm-b",1),(KEY,"load-warm-c",2),
                               (other,"load-hold-b",1),(KEY,"load-fail-a",0)] {
        bind_synthetic_account(&storage,tag,secret,cache,index);
    }
    let (release_tx,release_rx)=mpsc::channel();
    let mut held:Option<TcpStream>=None;
    let mock=DynamicMock::start(move |ordinal,mut stream|match ordinal {
        2=>{
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            write_chunk(&mut stream,&(created()+event(serde_json::json!({"type":"response.output_text.delta","delta":"holding B"})).as_str())).unwrap();
            held=Some(stream);
        }
        3=>write_synthetic_sse(&mut stream,&overload()),
        4=>{
            write_synthetic_sse(&mut stream,&completed(false));drop(stream);
            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            let mut held_stream=held.take().unwrap();
            write_chunk(&mut held_stream,&completed(false)).unwrap();
            held_stream.write_all(b"0\r\n\r\n").unwrap();
        }
        _=>write_synthetic_sse(&mut stream,&completed(false)),
    });
    let _upstream=EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{}/backend-api/codex",mock.addr));
    let _proxy=EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
    let server=TestServer::start();
    // Warm both backups so C has no cold-probe priority advantage. B remains
    // eligible with one live request: unlimited configured cap, recovery cap 2.
    for cache in ["load-warm-b","load-warm-c"] {
        let response=post_synthetic_body(&server.addr,KEY,body_with_cache(cache));
        assert!(response.1.contains("synthetic backup ok"),"{response:?}");
        wait_for_gateway_idle(&server.addr);
    }
    let _=mock.arrivals.recv_timeout(Duration::from_secs(2)).unwrap();
    let _=mock.arrivals.recv_timeout(Duration::from_secs(2)).unwrap();
    let address=server.addr.clone();
    let holding=thread::spawn(move||post_synthetic_body(&address,other,body_with_cache("load-hold-b")));
    let holder=mock.arrivals.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(holder.ends_with(&format!("synthetic-{tag}-1")));
    let result=post_synthetic_body(&server.addr,KEY,body_with_cache("load-fail-a"));
    release_tx.send(()).unwrap();
    assert!(result.1.contains("synthetic backup ok"),"{result:?}");
    assert_eq!(holding.join().unwrap().0,200);
    wait_for_gateway_idle(&server.addr);
    let seen=mock.finish();
    assert_eq!(seen.len(),5,"warm B/C, hold B, first A, one backup C");
    assert!(seen[3].ends_with(&format!("synthetic-{tag}-0")));
    assert!(seen[4].ends_with(&format!("synthetic-{tag}-2")),"must choose idle C while still-eligible B is busy: {seen:?}");
}

#[test]
fn backoff_revalidates_revoked_accounts_and_retains_original_error_without_a_backup() {
    let _lock=test_env_guard();
    let _dynamic=EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED","true");
    for revoke_all in [false,true] {
        let tag=if revoke_all {"live-revoke-all"} else {"live-revoke-b"};
        let dir=new_test_dir(tag);let db=dir.join("codexmanager.db");
        let _db=EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
        let storage=Storage::open(&db).unwrap();storage.init().unwrap();seed(&storage,tag,3);
        bind_synthetic_account(&storage,tag,KEY,"revoke-first-a",0);
        let (revoked_tx,revoked_rx)=mpsc::channel();let mock_db=db.clone();
        let mock=DynamicMock::start(move|ordinal,mut stream| {
            if ordinal==0 {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nRetry-After: 2\r\nConnection: close\r\n\r\n").unwrap();
                write_chunk(&mut stream,&overload()).unwrap();
                // Do not terminate the HTTP body. Gateway cancellation proves it
                // consumed the error and entered failover before the DB update.
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let closed=match stream.read(&mut [0u8;1]) {
                    Ok(0)=>true,
                    Err(err)=>matches!(err.kind(),std::io::ErrorKind::ConnectionReset|std::io::ErrorKind::ConnectionAborted),
                    _=>false,
                };
                assert!(closed,"gateway must cancel the primary before backoff");
                let changed=Storage::open(&mock_db).unwrap();
                changed.update_account_status(&format!("synthetic-{tag}-1"),"unavailable").unwrap();
                if revoke_all {changed.update_account_status(&format!("synthetic-{tag}-2"),"unavailable").unwrap();}
                revoked_tx.send(()).unwrap();
            } else {write_synthetic_sse(&mut stream,&completed(false));}
        });
        let _upstream=EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{}/backend-api/codex",mock.addr));
        let _proxy=EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
        let server=TestServer::start();
        let result=post_synthetic_body(&server.addr,KEY,body_with_cache("revoke-first-a"));
        revoked_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        wait_for_gateway_idle(&server.addr);
        let seen=mock.finish();
        assert!(seen[0].ends_with(&format!("synthetic-{tag}-0")));
        assert!(!seen.iter().any(|auth|auth.ends_with(&format!("synthetic-{tag}-1"))),"revoked B must never receive a POST");
        if revoke_all {
            assert_eq!(seen.len(),1,"no backup means no extra physical request");
            assert!(result.1.contains("server_is_overloaded") && result.1.contains(OVERLOAD),"original error must survive: {result:?}");
        } else {
            assert_eq!(seen.len(),2);
            assert!(seen[1].ends_with(&format!("synthetic-{tag}-2")));
            assert!(result.1.contains("synthetic backup ok"),"{result:?}");
        }
    }
}

#[test]
fn late_or_nonportable_overload_opens_circuit_for_the_next_bound_request() {
    let _lock=test_env_guard();
    let _dynamic=EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED","true");
    for visible in [true,false] {
        let tag=if visible {"late-visible-health"} else {"nonportable-health"};
        let dir=new_test_dir(tag);let db=dir.join("codexmanager.db");
        let _db=EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
        let storage=Storage::open(&db).unwrap();storage.init().unwrap();seed(&storage,tag,2);
        bind_synthetic_account(&storage,tag,KEY,"health-bound-a",0);
        let (observed_tx,observed_rx)=mpsc::channel();
        let mock=DynamicMock::start(move|ordinal,mut stream| {
            if ordinal==0 && visible {
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
                write_chunk(&mut stream,&(created()+event(serde_json::json!({"type":"response.output_text.delta","delta":"visible-before-capacity-failure"})).as_str())).unwrap();
                observed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                write_chunk(&mut stream,&overload()).unwrap();
                stream.write_all(b"0\r\n\r\n").unwrap();
            } else if ordinal==0 {write_synthetic_sse(&mut stream,&overload());}
            else {write_synthetic_sse(&mut stream,&completed(false));}
        });
        let _upstream=EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{}/backend-api/codex",mock.addr));
        let _proxy=EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
        let server=TestServer::start();
        let mut body=body_with_cache("health-bound-a");
        if !visible {body["previous_response_id"]="resp_account_scoped_opaque".into();}
        let first=if visible {
            let mut client=TcpStream::connect(&server.addr).unwrap();
            client.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
            let body=body.to_string();
            write!(client,"POST /v1/responses HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nAuthorization: Bearer {KEY}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",server.addr,body.len()).unwrap();
            let mut bytes=Vec::new();let mut acknowledged=false;
            loop {
                let mut buffer=[0u8;8192];let count=client.read(&mut buffer).unwrap();if count==0 {break;}
                bytes.extend_from_slice(&buffer[..count]);
                if !acknowledged && String::from_utf8_lossy(&bytes).contains("visible-before-capacity-failure") {
                    observed_tx.send(()).unwrap();acknowledged=true;
                }
            }
            assert!(acknowledged,"mock must wait until the client sees actual output");
            String::from_utf8(bytes).unwrap()
        } else {post_synthetic_body(&server.addr,KEY,body).1};
        assert!(first.contains("server_is_overloaded"),"{first}");
        wait_for_gateway_idle(&server.addr);
        assert_eq!(mock.seen.lock().unwrap().len(),1,"no replay after output or opaque context");
        let next=post_synthetic_body(&server.addr,KEY,body_with_cache("health-bound-a"));
        assert!(next.1.contains("synthetic backup ok"),"{next:?}");
        wait_for_gateway_idle(&server.addr);
        let seen=mock.finish();
        assert_eq!(seen.len(),2,"the later request should use one healthy account");
        assert!(seen[0].ends_with(&format!("synthetic-{tag}-0")));
        assert!(seen[1].ends_with(&format!("synthetic-{tag}-1")),"capacity observation must suppress even the bound account A: {seen:?}");
    }
}

#[test]
fn distinct_keys_share_burst_limit_and_twenty_percent_generation_retry_credit() {
    let _lock=test_env_guard();
    let _dynamic=EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED","true");
    let tag="shared-retry-credit";let dir=new_test_dir(tag);let db=dir.join("codexmanager.db");
    let _db=EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
    let storage=Storage::open(&db).unwrap();storage.init().unwrap();seed(&storage,tag,9);
    let secrets=(0..6).map(|index|format!("synthetic-shared-budget-key-{index}")).collect::<Vec<_>>();
    for (index,secret) in secrets.iter().enumerate() {
        insert_another_key(&storage,&format!("key-{tag}"),&format!("key-budget-{index}"),secret);
    }
    // Bucket starts with three retries, then earns one fifth per new logical
    // request. Six different Keys must still see the SAME bucket: 2,2,2,1,1,2
    // physical POSTs. The sixth request earns the fifth unit for one more retry.
    let mock=DynamicMock::start(move|ordinal,mut stream| {
        let success=matches!(ordinal,1|3|5|9);
        write_synthetic_sse(&mut stream,&if success {completed(false)} else {overload()});
    });
    let _upstream=EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{}/backend-api/codex",mock.addr));
    let _proxy=EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
    let server=TestServer::start();let mut total=0;
    for (index,secret) in secrets.iter().enumerate() {
        let result=post_synthetic(&server.addr,secret);
        wait_for_gateway_idle(&server.addr);
        let allowed=matches!(index,0|1|2|5);
        let expected=if allowed {2} else {1};total+=expected;
        assert_eq!(mock.seen.lock().unwrap().len(),total,"logical request {} must not obtain a private Key retry budget",index+1);
        if allowed {assert!(result.1.contains("synthetic backup ok"),"{result:?}");}
        else {assert!(result.1.contains("server_is_overloaded") && result.1.contains(OVERLOAD),"exhausted budget must retain upstream capacity error: {result:?}");}
    }
    assert_eq!(mock.finish().len(),10);
}

/// Omit the Content-Type line entirely when None. An empty header does not
/// reproduce the upstream behavior observed in the bounded live acceptance.
fn run_content_type_shape_case(
    tag: &str,
    content_type: Option<&'static str>,
    first_body: String,
    tool: bool,
) -> (u16,String,Vec<String>) {
    let _lock=test_env_guard();
    let _dynamic=EnvGuard::set("CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED","true");
    let dir=new_test_dir(tag);let db=dir.join("codexmanager.db");
    let _db=EnvGuard::set("CODEXMANAGER_DB_PATH",db.to_str().unwrap());
    let storage=Storage::open(&db).unwrap();storage.init().unwrap();seed(&storage,tag,3);
    let cache_key=format!("content-type-shape-{tag}");
    bind_synthetic_account(&storage,tag,KEY,&cache_key,0);
    let mock=DynamicMock::start(move|ordinal,mut stream| {
        if ordinal==0 {
            stream.write_all(b"HTTP/1.1 200 OK\r\n").unwrap();
            if let Some(content_type)=content_type {
                write!(stream,"Content-Type: {content_type}\r\n").unwrap();
            }
            write!(stream,"Content-Length: {}\r\nConnection: close\r\n\r\n{first_body}",first_body.len()).unwrap();
            stream.flush().unwrap();
        } else {write_synthetic_sse(&mut stream,&completed(tool));}
    });
    let _upstream=EnvGuard::set("CODEXMANAGER_UPSTREAM_BASE_URL",&format!("http://{}/backend-api/codex",mock.addr));
    let _proxy=EnvGuard::set("CODEXMANAGER_UPSTREAM_PROXY_URL","");
    let server=TestServer::start();
    let mut body=request("/v1/responses",true,tool);body["prompt_cache_key"]=cache_key.into();
    let response=post_synthetic_body(&server.addr,KEY,body);
    wait_for_gateway_idle(&server.addr);
    let attempts=mock.finish();
    assert!(attempts[0].ends_with(&format!("synthetic-{tag}-0")),"the established primary must receive the first POST");
    (response.0,response.1,attempts)
}

#[test]
fn missing_content_type_sse_overload_retries_one_backup() {
    let (status,body,attempts)=run_content_type_shape_case("no-header-sse-overload",None,overload(),false);
    assert_eq!(status,200,"{body}");
    assert_eq!(attempts.len(),2,"missing media-type header must not suppress safe SSE failover");
    assert_ne!(attempts[0],attempts[1],"only one different backup account is attempted");
    assert!(body.contains("response.completed") && body.contains("synthetic backup ok"),"{body}");
    assert!(!body.contains("server_is_overloaded") && !body.contains(OVERLOAD),"uncommitted first error must not leak downstream");
}

#[test]
fn json_content_type_sse_overload_retries_one_backup() {
    let (status,body,attempts)=run_content_type_shape_case("wrong-json-header-sse",Some("application/json"),overload(),false);
    assert_eq!(status,200,"{body}");
    assert_eq!(attempts.len(),2,"actual SSE framing must override an inaccurate JSON media type");
    assert_ne!(attempts[0],attempts[1]);
    assert!(body.contains("response.completed") && body.contains("synthetic backup ok"),"{body}");
    assert!(!body.contains("server_is_overloaded") && !body.contains(OVERLOAD));
}

#[test]
fn missing_or_json_content_type_sse_output_commits_before_later_overload() {
    for content_type in [None,Some("application/json")] {
        for tool in [false,true] {
            let tag=format!("header-shape-committed-{}-{}",if content_type.is_some(){"json"}else{"absent"},if tool{"tool"}else{"text"});
            let marker=if tool {"get_answer"} else {"header-shape-first-visible-text"};
            let prefix=if tool {
                event(serde_json::json!({"type":"response.output_item.added","output_index":0,
                    "item":{"type":"function_call","id":"fc_header_shape","call_id":"call_header_shape",
                            "name":marker,"arguments":"","status":"in_progress"}}))
            } else {
                event(serde_json::json!({"type":"response.output_text.delta","delta":marker}))
            };
            let first=created()+prefix.as_str()+overload().as_str();
            let (_,body,attempts)=run_content_type_shape_case(&tag,content_type,first,tool);
            assert_eq!(attempts.len(),1,"header sniffing must preserve the semantic output boundary: {tag}");
            assert!(body.contains(marker),"the primary text/tool event must remain observable: {tag}: {body}");
            assert!(body.contains("server_is_overloaded"),"the later error must remain observable: {tag}: {body}");
            assert!(!body.contains("synthetic backup ok"));
        }
    }
}

#[test]
fn unframed_json_capacity_error_is_not_mistaken_for_sse_failover() {
    for content_type in [None,Some("application/json")] {
        let tag=if content_type.is_some(){"real-json-error-header"}else{"real-json-error-no-header"};
        let first=serde_json::json!({"type":"response.failed","response":{"id":"resp_unframed_json",
            "status":"failed","error":{"code":"server_is_overloaded","message":OVERLOAD}}}).to_string();
        let (_,body,attempts)=run_content_type_shape_case(tag,content_type,first,false);
        assert_eq!(attempts.len(),1,"unframed JSON must not be promoted to the SSE-only replay path: {tag}");
        assert!(!body.contains("synthetic backup ok"));
    }
}

#[test]
fn plain_capacity_text_without_sse_framing_does_not_retry() {
    for content_type in [None,Some("text/plain")] {
        let tag=if content_type.is_some(){"plain-capacity-text-header"}else{"plain-capacity-text-no-header"};
        let first=format!("server_is_overloaded: {OVERLOAD}\nThis is a plain response, without an SSE data frame.\n");
        let (_,body,attempts)=run_content_type_shape_case(tag,content_type,first,false);
        assert_eq!(attempts.len(),1,"a textual overload mention is not structured SSE: {tag}");
        assert!(!body.contains("synthetic backup ok"));
    }
}
