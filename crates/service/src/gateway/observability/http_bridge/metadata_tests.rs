use super::{
    copy_upstream_response_headers, copy_upstream_response_headers_with_extras,
    upstream_response_metadata,
};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};

#[test]
fn upstream_response_metadata_extracts_debug_headers_and_content_flags() {
    let mut upstream = HeaderMap::new();
    upstream.insert("x-oai-request-id", HeaderValue::from_static(" req-123 "));
    upstream.insert("cf-ray", HeaderValue::from_static("cf-123"));
    upstream.insert(
        "x-openai-authorization-error",
        HeaderValue::from_static("quota exceeded"),
    );
    upstream.insert(
        "x-error-json",
        HeaderValue::from_static("{\"identity_error_code\":\"revoked\"}"),
    );
    upstream.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );

    let metadata = upstream_response_metadata(&upstream);

    assert_eq!(metadata.request_id.as_deref(), Some("req-123"));
    assert_eq!(metadata.cf_ray.as_deref(), Some("cf-123"));
    assert_eq!(metadata.auth_error.as_deref(), Some("quota exceeded"));
    assert_eq!(metadata.identity_error_code.as_deref(), Some("revoked"));
    assert_eq!(
        metadata.content_type.as_deref(),
        Some("text/event-stream; charset=utf-8")
    );
    assert!(metadata.is_sse);
    assert!(!metadata.is_json);
}

#[test]
fn copy_upstream_response_headers_filters_hop_by_hop_and_adds_trace_id() {
    let mut upstream = HeaderMap::new();
    upstream.insert("content-type", HeaderValue::from_static("application/json"));
    upstream.insert("transfer-encoding", HeaderValue::from_static("chunked"));
    upstream.insert("content-length", HeaderValue::from_static("12"));
    upstream.insert("connection", HeaderValue::from_static("keep-alive"));

    let headers = copy_upstream_response_headers(&upstream, Some(" trace-123 "));
    let names = headers
        .iter()
        .map(|header| header.field.as_str().as_str().to_ascii_lowercase())
        .collect::<Vec<_>>();

    assert!(names.contains(&"content-type".to_string()));
    assert!(names.contains(&crate::error_codes::TRACE_ID_HEADER_NAME.to_ascii_lowercase()));
    assert!(!names.contains(&"transfer-encoding".to_string()));
    assert!(!names.contains(&"content-length".to_string()));
    assert!(!names.contains(&"connection".to_string()));
}

#[test]
fn copy_upstream_response_headers_replaces_account_quota_headers_and_preserves_turn_state() {
    let mut upstream = HeaderMap::new();
    upstream.insert("content-type", HeaderValue::from_static("text/event-stream"));
    upstream.insert("x-codex-turn-state", HeaderValue::from_static("turn-keep-me"));
    upstream.insert("x-codex-primary-used-percent", HeaderValue::from_static("88"));
    upstream.insert("x-codex-primary-window-minutes", HeaderValue::from_static("300"));
    upstream.insert("x-codex-secondary-used-percent", HeaderValue::from_static("61"));
    upstream.insert("x-codex-credits-balance", HeaderValue::from_static("999.99"));
    upstream.insert("x-codex-active-limit", HeaderValue::from_static("codex_other"));

    let extras = vec![
        tiny_http::Header::from_bytes("x-codex-primary-used-percent", "22").expect("hdr"),
        tiny_http::Header::from_bytes("x-codex-primary-window-minutes", "10080").expect("hdr"),
        tiny_http::Header::from_bytes("x-codex-credits-balance", "186.40").expect("hdr"),
        tiny_http::Header::from_bytes("x-team-key-remaining-usd", "186.40").expect("hdr"),
    ];

    let headers = copy_upstream_response_headers_with_extras(&upstream, Some("tr-1"), &extras);
    let pairs: Vec<(String, String)> = headers
        .iter()
        .map(|h| {
            (
                h.field.as_str().as_str().to_ascii_lowercase(),
                h.value.as_str().to_string(),
            )
        })
        .collect();

    assert!(pairs.contains(&("x-codex-turn-state".to_string(), "turn-keep-me".to_string())));
    assert!(pairs.contains(&("x-codex-primary-used-percent".to_string(), "22".to_string())));
    assert!(pairs.contains(&("x-codex-primary-window-minutes".to_string(), "10080".to_string())));
    assert!(pairs.contains(&("x-codex-credits-balance".to_string(), "186.40".to_string())));
    assert!(pairs.contains(&("x-team-key-remaining-usd".to_string(), "186.40".to_string())));
    assert!(!pairs.iter().any(|(k, v)| k == "x-codex-primary-used-percent" && v == "88"));
    assert!(!pairs.iter().any(|(k, _)| k.starts_with("x-codex-secondary-")));
    assert!(!pairs.iter().any(|(k, _)| k == "x-codex-active-limit"));
}
