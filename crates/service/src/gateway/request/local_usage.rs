use chrono::{FixedOffset, TimeZone};
use codexmanager_core::storage::{now_ts, Storage, DEFAULT_KEY_WEEKLY_LIMIT_CENTS};
use tiny_http::Request;

const USAGE_RULE_WIDTH: usize = 60;
const USAGE_BAR_WIDTH: usize = 32;

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

    if wants_text(path) {
        let body = render_usage_text(
            key.name.as_deref().unwrap_or("-"),
            &key.status,
            used_cents,
            limit_cents,
            remaining_cents,
            next_release_at,
        );
        super::local_response::respond_local_text(
            request,
            &context,
            body,
            super::request_log::RequestLogUsage::default(),
        )?;
        return Ok(None);
    }

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

fn query_param<'a>(path: &'a str, name: &str) -> Option<&'a str> {
    let query = path.split_once('?')?.1;
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if key == name {
            Some(value)
        } else {
            None
        }
    })
}

fn wants_text(path: &str) -> bool {
    matches!(
        query_param(path, "format"),
        Some("text") | Some("pretty")
    ) || query_param(path, "pretty") == Some("1")
}

fn usd(cents: i64) -> String {
    format!("{:.2}", cents.max(0) as f64 / 100.0)
}

fn format_next_release(next_release_at: Option<i64>) -> String {
    let Some(timestamp) = next_release_at else {
        return "none (no billable usage in window)".to_string();
    };

    FixedOffset::east_opt(8 * 3600)
        .and_then(|tz| tz.timestamp_opt(timestamp, 0).single())
        .map(|dt| format!("{} (UTC+8)", dt.format("%Y-%m-%d %H:%M")))
        .unwrap_or_else(|| format!("{timestamp} (unix)"))
}

fn render_usage_text(
    label: &str,
    status: &str,
    used_cents: i64,
    limit_cents: i64,
    remaining_cents: i64,
    next_release_at: Option<i64>,
) -> String {
    let percent = if limit_cents > 0 {
        (used_cents.max(0) as f64 * 100.0 / limit_cents as f64).min(100.0)
    } else {
        0.0
    };
    let filled = ((USAGE_BAR_WIDTH as f64 * percent / 100.0).round() as usize)
        .min(USAGE_BAR_WIDTH);
    let bar = format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(USAGE_BAR_WIDTH - filled)
    );

    format!(
        concat!(
            "\n",
            "  GPT USAGE{}rolling 7d\n",
            "  {}\n",
            "  {:<12}{}\n",
            "  {:<12}{}\n",
            "\n",
            "  ${}  remaining\n",
            "  ${} used of ${}  |  {:.1}% of window\n",
            "  {}\n",
            "\n",
            "  {:<12}{}\n",
            "  {:<12}{}\n",
            "\n",
            "  {}\n",
            "\n"
        ),
        " ".repeat(USAGE_RULE_WIDTH - 11 - 10),
        "─".repeat(USAGE_RULE_WIDTH),
        "MEMBER",
        label,
        "STATUS",
        status.to_uppercase(),
        usd(remaining_cents),
        usd(used_cents),
        usd(limit_cents),
        percent,
        bar,
        "NEXT FREE",
        format_next_release(next_release_at),
        "WINDOW",
        "rolling 7 days",
        "─".repeat(USAGE_RULE_WIDTH),
    )
}

#[cfg(test)]
mod tests {
    use super::render_usage_text;

    #[test]
    fn text_usage_contains_visual_card() {
        let body = render_usage_text("肖明", "active", 0, 24_000, 24_000, None);
        assert!(body.contains("GPT USAGE"));
        assert!(body.contains("MEMBER"));
        assert!(body.contains("肖明"));
        assert!(body.contains("$240.00"));
        assert!(body.contains("0.0% of window"));
        assert!(body.contains("rolling 7 days"));
    }

    #[test]
    fn text_usage_shows_release_time() {
        let body = render_usage_text("肖明", "active", 12_000, 24_000, 12_000, Some(1_800_000_000));
        assert!(body.contains("$120.00"));
        assert!(body.contains("50.0% of window"));
        assert!(body.contains("UTC+8"));
    }
}
