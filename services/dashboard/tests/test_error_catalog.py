import json
import sqlite3
import pytest

from dashboard.collector import collect, public_id
from dashboard.error_catalog import classify_failure, public_catalog_entries, safe_failure_message
from dashboard.models import Snapshot


@pytest.mark.parametrize(
    ("status_code", "error", "expected_sig", "expected_layer"),
    [
        (499, "连接中断（可能是网络波动或客户端主动取消）", "client_cancelled", "client_cancelled"),
        (500, "读取用量失败(read api key weekly usage failed: database is locked)", "gateway_storage_error", "gateway_pool"),
        (503, "无可用账号(no available account); kind=no_available_account_exhausted; attempted=auth0|abc::cgpt=123; last_attempt=generation retry budget exhausted: shared pool limit", "pool_exhausted", "gateway_pool"),
        (429, "API Key 周额度已用尽（滚动 7 天）(api key weekly quota exhausted: used 24000 cents, limit 24000 cents)", "key_weekly_quota_exhausted", "member_policy"),
        (429, "请求触发单 Key 单并发限制", "key_concurrency_limit", "member_policy"),
        (429, "account temporarily unavailable; kind=bound_account_cooldown; retry_after=18s; account_id=auth0|secret", "pool_cooldown", "gateway_pool"),
        (403, "API Key 已禁用(api key disabled)", "key_disabled", "member_policy"),
        (401, "缺少 API Key(missing api key)", "auth_missing_or_invalid", "member_policy"),
        (502, "code=server_is_overloaded Our servers are currently overloaded", "upstream_overloaded", "upstream_openai"),
        (502, "code=cyber_policy type=invalid_request flagged for cybersecurity risk", "upstream_cyber_policy", "upstream_openai"),
        (504, "request timed out after 120s", "upstream_timeout", "upstream_openai"),
        (502, "upstream error: error sending request for url (https://chatgpt.com/secret)", "upstream_stream_interrupted", "upstream_openai"),
        (404, "model_not_found: gpt-6.1-terra", "model_or_param_invalid", "member_policy"),
        (500, "internal panic in router", "gateway_internal_error", "gateway_pool"),
        (502, "code=server_error type=server_error An error occurred while processing", "upstream_server_error", "upstream_openai"),
        (418, "unexpected odd response", "unknown_error", "upstream_openai"),
    ],
)
def test_classify_failure_signatures_and_layers(status_code, error, expected_sig, expected_layer):
    diag = classify_failure(status_code, error)
    assert diag["signature"] == expected_sig
    assert diag["layer"] == expected_layer
    assert diag["meaning"]
    assert diag["action"]


def test_safe_failure_message_strips_secrets_urls_and_account_ids():
    canary = "SECRET-CANARY-VALUE-12345"
    raw = (
        f"无可用账号(no available account); kind=no_available_account_exhausted; "
        f"attempted=auth0|YPycGZB::cgpt=e0a6,google-oauth2|11418::cgpt=9605; "
        f"url=https://private.internal/path {canary} user@example.com sk-1234567890abcdef"
    )
    msg = safe_failure_message(503, raw)
    assert canary not in msg
    assert "auth0|" not in msg
    assert "google-oauth2" not in msg
    assert "private.internal" not in msg
    assert "user@example.com" not in msg
    assert "sk-1234567890" not in msg
    assert "kind=no_available_account_exhausted" in msg
    assert "已尝试 2 个共享账号" in msg


def test_collector_enriches_daily_signatures_model_weather_and_fractional_cents(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("ALTER TABLE request_logs ADD COLUMN key_id TEXT")
        db.execute("ALTER TABLE request_logs ADD COLUMN account_id TEXT")
        db.execute("ALTER TABLE request_logs ADD COLUMN model TEXT")
        db.execute(
            "INSERT INTO request_logs(id,error,upstream_url,status_code,duration_ms,first_response_ms,created_at,key_id,account_id,model) VALUES(?,?,?,?,?,?,?,?,?,?)",
            (100, "code=server_is_overloaded " + sample["canary"], sample["canary"], 502, 400, None, sample["day"] + 20, "internal-key-a", "internal-account", "gpt-6-astra"),
        )
        db.execute(
            "INSERT INTO request_logs(id,error,upstream_url,status_code,duration_ms,first_response_ms,created_at,key_id,account_id,model) VALUES(?,?,?,?,?,?,?,?,?,?)",
            (101, None, sample["canary"], 200, 1400, 350, sample["day"] + 25, "internal-key-a", "internal-account", "gpt-6-astra"),
        )
        db.execute(
            "INSERT INTO request_token_stats VALUES(?,?,?,?,?,?,?,?,?,?)",
            (101, "internal-key-a", "gpt-6-astra", 1, sample["day"] + 25, 100, 0, 50, 150, 0.27),
        )
    snap = collect(sample["config"], sample["now"])
    Snapshot.model_validate(snap)

    encoded = json.dumps(snap, ensure_ascii=False)
    assert sample["canary"] not in encoded

    # Verify fractional weekly cents accuracy (0.10 + 0.20 + 0.27 = 0.57 => 57 cents) and next release
    key_a = next(k for k in snap["keys"] if k["id"] == public_id("key", "internal-key-a", sample["config"]["id_secret"]))
    assert key_a["weekly_used_cents"] == 57
    assert key_a["weekly_remaining_cents"] == 24000 - 57
    assert key_a["next_release_at"] is not None
    assert key_a["next_release_cents"] > 0

    # Verify daily signature explanation & effective_success_rate
    today_h = snap["request_health"]["today"]
    assert today_h["signatures"]
    sig_names = {s["signature"] for s in today_h["signatures"]}
    assert {"no_available_account", "pool_exhausted"} & sig_names
    assert "upstream_overloaded" in sig_names
    assert today_h["effective_success_rate"] >= today_h["success_rate"]

    # Verify model weather and safe account label in failures (never canary accounts.label)
    assert any(w["model"] == "gpt-6-astra" and w["today_overload_count"] >= 1 for w in snap["model_weather"])
    acct_failure = next(f for f in snap["recent_failures"] if f["signature"] == "upstream_overloaded")
    assert acct_failure["account_label"] == "账号 01"
    assert acct_failure["key_label"] == "测试 A / Codex CLI"


def test_catalog_entries_complete():
    entries = public_catalog_entries()
    assert len(entries) == 16
    assert len({e["signature"] for e in entries}) == 16
