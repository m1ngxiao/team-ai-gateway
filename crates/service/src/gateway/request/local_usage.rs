use codexmanager_core::storage::{now_ts, Storage, DEFAULT_KEY_WEEKLY_LIMIT_CENTS};
use tiny_http::Request;

/// Respond to `GET /v1/usage` with the authenticated key's rolling quota.
///
/// This is a local read-only endpoint: it never selects an upstream account and
/// never consumes quota.
pub(super) fn maybe_respond_local_usage(
    request: Request,
    trace_id: &str,
    key_id: &str,
    protocol_type: &str,
    original_path: &str,
    path: &str,
    response_adapter: super::ResponseAdapter,
    request_method: &str,
    model_for_log: Option<&str>,
    reasoning_for_log: Option<&str>,
    storage: &Storage,
) -> Result<Option<Request>, String> {
    let is_usage = path == "/v1/usage" || path.starts_with("/v1/usage?");
    if !is_usage {
        return Ok(Some(request));
    }

    let context = super::local_response::LocalResponseContext {
        trace_id,
        key_id,
        protocol_type,
        original_path,
        path,
        response_adapter,
        request_method,
        model_for_log,
        reasoning_for_log,
        storage,
    };

    if !request_method.eq_ignore_ascii_case("GET") {
        super::local_response::respond_local_terminal_error(
            request,
            &context,
            405,
            "usage endpoint only supports GET".to_string(),
        )?;
        return Ok(None);
    }

    let key = match storage.find_api_key_by_id(key_id) {
        Ok(Some(key)) => key,
        Ok(None) => {
            super::local_response::respond_local_terminal_error(
                request,
                &context,
                404,
                "api key not found".to_string(),
            )?;
            return Ok(None);
        }
        Err(err) => {
            super::local_response::respond_local_terminal_error(
                request,
                &context,
                503,
                format!("read api key failed: {err}"),
            )?;
            return Ok(None);
        }
    };

    let now = now_ts();
    let used_cents = match storage.key_usage_cents_rolling_7d(key_id, now) {
        Ok(value) => value.max(0),
        Err(err) => {
            super::local_response::respond_local_terminal_error(
                request,
                &context,
                503,
                format!("read weekly usage failed: {err}"),
            )?;
            return Ok(None);
        }
    };
    let next_release_at = match storage.key_next_release_at(key_id, now) {
        Ok(value) => value,
        Err(err) => {
            super::local_response::respond_local_terminal_error(
                request,
                &context,
                503,
                format!("read usage release time failed: {err}"),
            )?;
            return Ok(None);
        }
    };

    let limit_cents = DEFAULT_KEY_WEEKLY_LIMIT_CENTS;
    let remaining_cents = (limit_cents - used_cents).max(0);
    let body = serde_json::json!({
        "key": {
            "id": key.id,
            "label": key.name,
            "status": key.status,
        },
        "window": "rolling_7d",
        "used_cents": used_cents,
        "limit_cents": limit_cents,
        "remaining_cents": remaining_cents,
        "used_usd": format!("{:.4}", used_cents as f64 / 100.0),
        "limit_usd": format!("{:.2}", limit_cents as f64 / 100.0),
        "remaining_usd": format!("{:.4}", remaining_cents as f64 / 100.0),
        "next_release_at": next_release_at,
    })
    .to_string();

    super::local_response::respond_local_json(
        request,
        &context,
        body,
        super::request_log::RequestLogUsage::default(),
    )?;
    Ok(None)
}
