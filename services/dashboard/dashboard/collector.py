"""Read-only, schema-pinned SQLite exporter. It never selects secret columns."""

import argparse
import hashlib
import hmac
import json
import logging
from logging.handlers import RotatingFileHandler
import math
import os
import re
import sqlite3
import tempfile
import time
from contextlib import contextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path

from .models import Snapshot
from .account_details import credits_details, email_from_label, positive_int, timestamp
from .catalog import CATALOG_COLUMNS, collect_catalog, key_metadata, public_text
from .error_catalog import ERROR_LAYERS, SEVERITY_RANK, classify_failure, safe_failure_message

LOG = logging.getLogger("collector")
TZ = timezone(timedelta(hours=8))
FIELDS = ("requests", "input_tokens", "cached_tokens", "output_tokens", "total_tokens", "estimated_usd")
REQUEST_HEALTH_HISTORY_DAYS = 14
REQUEST_ERROR_KINDS = (
    "no_available_account",
    "rate_limited",
    "auth_error",
    "timeout",
    "client_cancelled",
    "network_error",
    "upstream_overloaded",
    "invalid_request",
    "upstream_error",
    "unknown_error",
)
CATEGORY_FALLBACK_LAYER = {
    "no_available_account": "gateway_pool",
    "rate_limited": "gateway_pool",
    "auth_error": "member_policy",
    "invalid_request": "member_policy",
    "client_cancelled": "client_cancelled",
    "timeout": "upstream_openai",
    "network_error": "upstream_openai",
    "upstream_overloaded": "upstream_openai",
    "upstream_error": "upstream_openai",
    "unknown_error": "upstream_openai",
}
ALLOWED_COLUMNS = {
    **CATALOG_COLUMNS,
    "accounts": {"id", "status", "created_at", "updated_at", "label", "sort", "group_name"},
    "claude_subscription_accounts": {"id", "email", "subscription_type", "status", "sort", "updated_at"},
    "claude_subscription_usage": {"account_id", "five_hour_used_percent", "five_hour_resets_at",
                                  "seven_day_used_percent", "seven_day_resets_at", "captured_at",
                                  "last_attempt_at", "next_attempt_at", "last_error"},
    "account_subscriptions": {"account_id", "account_plan_type", "plan_type", "expires_at", "renews_at"},
    "account_quota_capacity_overrides": {"account_id", "primary_window_tokens", "secondary_window_tokens"},
    "account_proxy_settings": {"account_id", "enabled"},
    "usage_snapshots": {"id", "account_id", "used_percent", "window_minutes", "resets_at",
                        "secondary_used_percent", "secondary_window_minutes", "secondary_resets_at", "captured_at", "credits_json"},
    "api_keys": {"id", "name", "status", "created_at", "last_used_at", "rotation_strategy", "model_slug", "account_group_filter", "upstream_provider"},
    "api_key_profiles": {"key_id", "protocol_type", "default_model"},
    "api_key_quota_limits": {"key_id", "quota_limit_tokens"},
    "request_logs": {"id", "key_id", "account_id", "model", "status_code", "error", "duration_ms", "first_response_ms", "created_at"},
    "key_usage_hourly": {"key_id", "hour_start", "cost_cents"},
    "key_quota_state": {"key_id", "epoch_at"},
    "request_token_stats": {"request_log_id", "key_id", "account_id", "model", "usage_included", "created_at", "actual_source_kind",
                            "input_tokens", "cached_input_tokens", "output_tokens", "reasoning_output_tokens",
                            "total_tokens", "estimated_cost_usd"},
    "request_token_stat_hourly_rollups": {"key_id", "model", "bucket_start", "bucket_end", "request_count", "actual_source_kind",
                                          "input_tokens", "cached_input_tokens", "output_tokens", "total_tokens", "estimated_cost_usd"},
    "request_token_stat_rollups": {"key_id", "model", "source_rows", "input_tokens",
                                    "cached_input_tokens", "output_tokens", "total_tokens", "estimated_cost_usd"},
}


def request_error_category(status_code, error):
    """Reduce a failed request to a safe, useful operational category."""
    code = status_code if type(status_code) is int and 100 <= status_code <= 599 else None
    text = str(error or "").casefold()
    if code in {401, 403} or any(marker in text for marker in (
        "missing api key", "invalid api key", "unauthorized", "forbidden",
        "缺少 api key", "鉴权失败", "认证失败", "配置错误：未配置",
    )):
        return "auth_error"
    if any(marker in text for marker in (
        "no available account", "no_available_account", "无可用账号",
        "account pool exhausted", "no healthy account",
    )):
        return "no_available_account"
    if code == 429 or any(marker in text for marker in (
        "rate limit", "rate_limited", "too many requests", "account temporarily unavailable",
        "限流", "账号暂时不可用",
    )):
        return "rate_limited"
    if code in {408, 504} or any(marker in text for marker in (
        "timeout", "timed out", "deadline exceeded", "超时",
    )):
        return "timeout"
    if code == 499 or any(marker in text for marker in (
        "broken pipe", "client disconnected", "client cancelled", "client canceled",
        "downstream disconnect", "response write failed", "客户端取消", "客户端中断",
    )):
        return "client_cancelled"
    if any(marker in text for marker in (
        "server_is_overloaded", "overloaded", "over capacity", "服务繁忙", "上游繁忙",
    )):
        return "upstream_overloaded"
    if any(marker in text for marker in (
        "transport error", "error sending request", "connection reset", "connection refused",
        "unexpected eof", "network error", "dns error", "连接中断", "网络错误", "连接失败",
    )):
        return "network_error"
    if code is not None and 400 <= code <= 499:
        return "invalid_request"
    if code is not None and 500 <= code <= 599:
        return "upstream_error"
    return "unknown_error"


def failure_message(status_code, error):
    """Build a strictly allowlisted failure message without exposing raw URLs, IDs, or secrets."""
    return safe_failure_message(status_code, error)


def collect_key_weekly_cents(connection, end):
    try:
        rows = connection.execute("""
          SELECT t.key_id, SUM(COALESCE(t.estimated_cost_usd, 0)) AS used_usd
          FROM request_token_stats t
          LEFT JOIN key_quota_state s ON s.key_id = t.key_id
          WHERE t.usage_included = 1
            AND t.key_id IS NOT NULL
            AND t.created_at >= MAX(?, COALESCE(s.epoch_at, 0))
          GROUP BY t.key_id
        """, (end - 604800,)).fetchall()
    except sqlite3.OperationalError:
        try:
            rows = connection.execute("""
              SELECT t.key_id, SUM(COALESCE(t.estimated_cost_usd, 0)) AS used_usd
              FROM request_token_stats t
              WHERE t.usage_included = 1
                AND t.key_id IS NOT NULL
                AND t.created_at >= ?
              GROUP BY t.key_id
            """, (end - 604800,)).fetchall()
        except sqlite3.OperationalError:
            return {}
    return {row["key_id"]: max(0, round(safe_number(row["used_usd"], floating=True) * 100)) for row in rows}


def collect_key_next_release(connection, end, key_weekly_cents=None):
    """Return the oldest meaningful charged request per key and when it leaves the 7d window."""
    try:
        try:
            rows = connection.execute("""
              SELECT t.key_id, MIN(t.created_at) AS next_ts
              FROM request_token_stats t
              LEFT JOIN key_quota_state s ON s.key_id = t.key_id
              WHERE t.usage_included = 1
                AND t.key_id IS NOT NULL
                AND COALESCE(t.estimated_cost_usd, 0) >= 0.005
                AND t.created_at >= MAX(?, COALESCE(s.epoch_at, 0))
              GROUP BY t.key_id
            """, (end - 604800,)).fetchall()
        except sqlite3.OperationalError:
            rows = connection.execute("""
              SELECT t.key_id, MIN(t.created_at) AS next_ts
              FROM request_token_stats t
              WHERE t.usage_included = 1
                AND t.key_id IS NOT NULL
                AND COALESCE(t.estimated_cost_usd, 0) >= 0.005
                AND t.created_at >= ?
              GROUP BY t.key_id
            """, (end - 604800,)).fetchall()
        result = {}
        for row in rows:
            key_id = row["key_id"]
            if key_weekly_cents is not None and key_weekly_cents.get(key_id, 0) <= 0:
                continue
            next_ts = safe_number(row["next_ts"])
            if next_ts <= 0:
                continue
            cost = connection.execute("""
              SELECT SUM(COALESCE(estimated_cost_usd, 0)) AS used_usd
              FROM request_token_stats
              WHERE key_id = ? AND created_at = ? AND usage_included = 1
            """, (key_id, next_ts)).fetchone()
            cents = max(1, round(safe_number(cost["used_usd"], floating=True) * 100) if cost else 0)
            result[key_id] = (next_ts + 604800, cents)
        return result
    except sqlite3.OperationalError:
        return {}


def collect_account_weekly_cents(connection, end):
    try:
        rows = connection.execute("""
          SELECT account_id, SUM(COALESCE(estimated_cost_usd, 0)) AS used_usd
          FROM request_token_stats
          WHERE usage_included = 1 AND created_at >= ? AND account_id IS NOT NULL
          GROUP BY account_id
        """, (end - 604800,)).fetchall()
    except sqlite3.OperationalError:
        return {}
    return {row["account_id"]: max(0, int(round(safe_number(row["used_usd"], floating=True) * 100)))
            for row in rows}


def _safe_public_model(value, public_models):
    if isinstance(value, str) and value in public_models:
        return value
    if isinstance(value, str) and value.strip():
        return "other"
    return None


def collect_recent_failures(connection, start, end, secret, account_labels, key_labels, public_models):
    include_key = include_account = include_model = include_duration = True
    while True:
        key_expr = "key_id" if include_key else "NULL AS key_id"
        acct_expr = "account_id" if include_account else "NULL AS account_id"
        model_expr = "model" if include_model else "NULL AS model"
        dur_expr = "duration_ms" if include_duration else "NULL AS duration_ms"
        sql = f"""
          SELECT created_at, status_code, error, {model_expr}, {dur_expr}, {key_expr}, {acct_expr}
          FROM request_logs
          WHERE created_at >= ? AND created_at < ? AND NOT (status_code >= 200 AND status_code <= 299)
          ORDER BY created_at DESC LIMIT 200
        """
        try:
            rows = connection.execute(sql, (start, end)).fetchall()
            break
        except sqlite3.OperationalError as exc:
            msg = str(exc)
            if msg == "no such table: request_logs":
                return []
            if include_key and "no such column: key_id" in msg:
                include_key = False
                continue
            if include_account and "no such column: account_id" in msg:
                include_account = False
                continue
            if include_model and "no such column: model" in msg:
                include_model = False
                continue
            if include_duration and "no such column: duration_ms" in msg:
                include_duration = False
                continue
            raise

    failures = []
    for row in rows:
        code = row["status_code"] if type(row["status_code"]) is int and 100 <= row["status_code"] <= 599 else 500
        occurred_at = safe_number(row["created_at"])
        diag = classify_failure(code, row["error"])
        raw_key = row["key_id"]
        raw_acct = row["account_id"]
        key_pid = public_id("key", raw_key, secret) if isinstance(raw_key, str) and raw_key else None
        failures.append({
            "occurred_at": occurred_at,
            "date": datetime.fromtimestamp(occurred_at, TZ).date().isoformat(),
            "status_code": code,
            "category": request_error_category(row["status_code"], row["error"]),
            "signature": diag["signature"],
            "layer": diag["layer"],
            "severity": diag["severity"],
            "title": diag["title"],
            "meaning": diag["meaning"],
            "action": diag["action"],
            "model": _safe_public_model(row["model"], public_models),
            "key_id": key_pid,
            "key_label": key_labels.get(raw_key) if isinstance(raw_key, str) else None,
            "account_label": account_labels.get(raw_acct) if isinstance(raw_acct, str) else None,
            "duration_ms": safe_number(row["duration_ms"]) if row["duration_ms"] is not None else None,
            "message": safe_failure_message(code, row["error"], diag),
        })
    return failures


def _percentile(values, ratio):
    if not values:
        return None
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, int(round((len(ordered) - 1) * ratio))))
    return float(round(ordered[index], 1))


def _speed_point(rows):
    samples = len(rows)
    output_tokens = sum(row["output_tokens"] for row in rows)
    e2e = [row["e2e"] for row in rows if row["e2e"] is not None]
    decode = [row["decode"] for row in rows if row["decode"] is not None]
    return {
        "samples": samples,
        "output_tokens": output_tokens,
        "e2e_p50": _percentile(e2e, 0.5),
        "e2e_p90": _percentile(e2e, 0.9),
        "decode_p50": _percentile(decode, 0.5),
        "decode_p90": _percentile(decode, 0.9),
    }


def collect_speed(connection, end, public_models=None):
    """Approximate generation speed from successful requests with recorded durations."""
    try:
        rows = connection.execute("""
          SELECT t.model AS model, t.output_tokens AS output_tokens,
                 COALESCE(t.reasoning_output_tokens, 0) AS reasoning_tokens,
                 l.duration_ms AS duration_ms, l.first_response_ms AS first_response_ms
          FROM request_token_stats t
          JOIN request_logs l ON l.id = t.request_log_id
          WHERE t.created_at < ? AND t.created_at >= ?
            AND t.usage_included = 1 AND t.output_tokens > 0
            AND t.model <> 'codex-auto-review'
            AND l.status_code >= 200 AND l.status_code <= 299
            AND l.duration_ms > 0
        """, (end, end - 604800)).fetchall()
    except sqlite3.OperationalError:
        return None
    points = []
    for row in rows:
        output = max(0, int(safe_number(row["output_tokens"])))
        if output <= 0:
            continue
        reasoning = min(output, max(0, int(safe_number(row["reasoning_tokens"]))))
        visible = max(0, output - reasoning)
        duration_ms = max(1, int(safe_number(row["duration_ms"])))
        duration_s = duration_ms / 1000.0
        first_ms = row["first_response_ms"]
        first_ms = max(0, int(safe_number(first_ms))) if first_ms is not None else 0
        decode_s = max(0.0, (duration_ms - first_ms) / 1000.0) if first_ms > 0 else 0.0
        e2e = output / duration_s if duration_s > 0 else None
        decode = visible / decode_s if decode_s > 0 and visible > 0 else None
        if public_models is not None:
            model_name = _safe_public_model(row["model"], public_models) or "other"
        else:
            model_name = public_text(row["model"], "other", maximum=64)
        points.append({
            "model": model_name,
            "output_tokens": output,
            "duration_s": duration_s,
            "decode_s": decode_s if decode is not None else 0.0,
            "e2e": e2e,
            "decode": decode,
        })
    overall = _speed_point(points)
    by_model = {}
    for point in points:
        by_model.setdefault(point["model"], []).append(point)
    models = []
    for model, items in sorted(by_model.items(), key=lambda pair: len(pair[1]), reverse=True)[:50]:
        models.append({"model": model, **_speed_point(items)})
    return {**overall, "models": models}


def read_authorizer(action, table, column, database, trigger):
    if action == sqlite3.SQLITE_READ:
        return sqlite3.SQLITE_OK if database == "main" and column in ALLOWED_COLUMNS.get(table, set()) else sqlite3.SQLITE_DENY
    if action in {sqlite3.SQLITE_SELECT, sqlite3.SQLITE_TRANSACTION}:
        return sqlite3.SQLITE_OK
    if action == sqlite3.SQLITE_FUNCTION:
        return sqlite3.SQLITE_OK if column.lower() in {
            "sum", "count", "coalesce", "max", "min", "request_error_category",
        } else sqlite3.SQLITE_DENY
    return sqlite3.SQLITE_DENY


@contextmanager
def source_connection(path: Path):
    path = path.resolve(strict=True)
    connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=2)
    try:
        connection.row_factory = sqlite3.Row
        connection.create_function("request_error_category", 2, request_error_category, deterministic=True)
        connection.execute("PRAGMA query_only=ON")
        connection.set_authorizer(read_authorizer)
        deadline = time.monotonic() + 8
        connection.set_progress_handler(lambda: int(time.monotonic() > deadline), 10000)
        connection.execute("BEGIN")
        yield connection
    finally:
        connection.close()


def zero():
    return {field: (0.0 if field == "estimated_usd" else 0) for field in FIELDS}


def merge(target, source):
    for field in FIELDS:
        target[field] += source[field]


def safe_number(value, *, floating=False):
    if value is None:
        return 0.0 if floating else 0
    number = float(value)
    if not math.isfinite(number):
        raise ValueError("nonfinite aggregate")
    return max(0.0, number) if floating else max(0, int(value))


def optional_timestamp(value):
    return int(value) if isinstance(value, (int, float)) and math.isfinite(value) and value > 0 else None


def public_id(kind, internal_id, secret):
    digest = hmac.new(bytes.fromhex(secret), f"{kind}:{internal_id}".encode(), hashlib.sha256).hexdigest()
    return f"{kind}-{digest[:12]}"


def safe_label(value, fallback):
    # Only administrator-approved aliases, never raw database names or emails.
    if isinstance(value, str) and re.fullmatch(r"[\w .·()\-]{1,32}", value) and not re.search(r"[a-fA-F0-9]{24}", value):
        return value
    return fallback


def safe_status(value):
    if value in {"active", "enabled", "available", "normal"}:
        return "enabled"
    if value in {"disabled", "inactive", "paused"}:
        return "disabled"
    if value in {"unavailable", "expired", "error", "invalid", "banned"}:
        return "unavailable"
    return "unknown"


def claude_plan(value):
    value = str(value or "").strip().lower().replace("-", "_")
    if value.startswith("claude_"):
        value = value.removeprefix("claude_")
    for plan in ("pro", "max", "team", "enterprise", "free"):
        if value == plan or value.startswith(plan + "_"):
            return plan
    return "unknown"


def claude_status(value):
    return {"active": "enabled", "disabled": "disabled", "needs_login": "needs_login"}.get(value, "unknown")


def claude_usage_error(value):
    """Expose only a small error category, never an upstream response or URL."""
    if not value:
        return None
    code = str(value).strip().lower().replace("-", "_").replace(" ", "_")
    if code in {"rate_limited", "http_429", "429"}:
        return "rate_limited"
    if code in {"auth_error", "token_refresh_error", "unauthorized", "forbidden", "http_401", "http_403", "401", "403"}:
        return "auth_error"
    if code in {"timeout", "timed_out", "request_timeout"}:
        return "timeout"
    if code in {"network_error", "transport_error", "connection_error"}:
        return "network_error"
    if code in {"invalid_response", "parse_error", "invalid_json"}:
        return "invalid_response"
    if code == "upstream_error" or re.fullmatch(r"http_5\d\d", code):
        return "upstream_error"
    return "query_failed"


def claude_quota_window(used, resets_at, minutes):
    """The usage endpoint reports percentage used; the dashboard shows remaining."""
    remaining = None
    if isinstance(used, (int, float)) and not isinstance(used, bool) and math.isfinite(used) and 0 <= used <= 100:
        remaining = float(100 - used)
    return {"minutes": minutes, "remaining_percent": remaining, "resets_at": timestamp(resets_at)}


def group_identity(value, secret):
    # Group names have the same trimmed, case-sensitive identity as gateway routing.
    # Validate display text separately so redacted names never merge unrelated groups.
    if value is None or isinstance(value, str) and not value.strip():
        return None, None
    raw = value.strip() if isinstance(value, str) else repr(value)
    gid = public_id("group", "named:" + raw, secret)
    return gid, public_text(value, "分组 " + gid.removeprefix("group-"))


def collect_key_groups(raw_keys, periods, secret):
    groups, membership = {}, {}

    def ensure(gid, label, kind):
        return groups.setdefault(gid, {"id": gid, "label": label, "kind": kind, "key_count": 0,
                                       "usage": {name: zero() for name in periods}})

    for key in raw_keys:
        gid, label = group_identity(key["account_group_filter"], secret)
        if gid is None:
            gid = public_id("group", "special:unrestricted", secret)
            label, kind = "全部分组（未限制）", "unrestricted"
        else:
            kind = "named"
        ensure(gid, label, kind)["key_count"] += 1
        membership[key["id"]] = gid

    for period_name, (by_key, _, _, _) in periods.items():
        for key_id, usage in by_key.items():
            gid = membership.get(key_id)
            if gid is None:
                # Preserve deleted/unknown-key archives in a separate bucket so
                # every group sum still reconciles with the independent totals.
                if not any(usage.values()):
                    continue
                gid = public_id("group", "special:unattributed", secret)
                ensure(gid, "已删除或未归属 Key", "unattributed")
            merge(groups[gid]["usage"][period_name], usage)
    return list(groups.values())


def quota_window(row, prefix=""):
    used = row[prefix + "used_percent"]
    remaining = None
    if isinstance(used, (int, float)) and math.isfinite(used):
        remaining = float(max(0, min(100, 100 - used)))
    return {"minutes": optional_timestamp(row[prefix + "window_minutes"]),
            "remaining_percent": remaining,
            "resets_at": optional_timestamp(row[prefix + "resets_at"])}


def aggregate(connection, start: int | None, end: int, public_models=frozenset()):
    """Raw + disjoint archives; only recorded actual sources count toward a pool."""
    raw_filter = "t.created_at < ?" if start is None else "t.created_at >= ? AND t.created_at < ?"
    hourly_filter = "h.bucket_end <= ?" if start is None else "h.bucket_start >= ? AND h.bucket_end <= ?"
    args = [end, end] if start is None else [start, end, start, end]
    sql = f"""
      SELECT t.key_id, t.model,t.actual_source_kind AS actual_source_kind,
        COUNT(t.request_log_id) AS requests,
        SUM(CASE WHEN t.usage_included=1 THEN MAX(COALESCE(t.input_tokens,0),0) ELSE 0 END) AS input_tokens,
        SUM(CASE WHEN t.usage_included=1 THEN MAX(COALESCE(t.cached_input_tokens,0),0) ELSE 0 END) AS cached_tokens,
        SUM(CASE WHEN t.usage_included=1 THEN MAX(COALESCE(t.output_tokens,0),0) ELSE 0 END) AS output_tokens,
        SUM(CASE WHEN t.usage_included<>1 THEN 0 ELSE MAX(COALESCE(t.total_tokens,
          COALESCE(t.input_tokens,0)-COALESCE(t.cached_input_tokens,0)+COALESCE(t.output_tokens,0)),0) END) AS total_tokens,
        SUM(CASE WHEN t.usage_included=1 THEN MAX(COALESCE(t.estimated_cost_usd,0),0) ELSE 0 END) AS estimated_usd
      FROM request_token_stats t WHERE {raw_filter} GROUP BY t.key_id,t.model,t.actual_source_kind
      UNION ALL
      SELECT h.key_id,h.model,h.actual_source_kind AS actual_source_kind,SUM(h.request_count),SUM(h.input_tokens),SUM(h.cached_input_tokens),
        SUM(h.output_tokens),SUM(h.total_tokens),SUM(h.estimated_cost_usd)
      FROM request_token_stat_hourly_rollups h WHERE {hourly_filter} GROUP BY h.key_id,h.model,h.actual_source_kind
    """
    if start is None:
        sql += """ UNION ALL
          SELECT l.key_id,l.model,NULL AS actual_source_kind,SUM(l.source_rows),SUM(l.input_tokens),SUM(l.cached_input_tokens),
            SUM(l.output_tokens),SUM(l.total_tokens),SUM(l.estimated_cost_usd)
          FROM request_token_stat_rollups l GROUP BY l.key_id,l.model"""
    for _ in range(3):
        try:
            rows = connection.execute(sql, args)
            break
        except sqlite3.OperationalError as exc:
            if str(exc) == "no such column: t.actual_source_kind":
                sql = sql.replace("t.actual_source_kind", "NULL")
            elif str(exc) == "no such column: h.actual_source_kind":
                sql = sql.replace("h.actual_source_kind", "NULL")
            else:
                raise
    by_key, by_model, total = {}, {}, zero()
    by_pool = {"openai": zero(), "claude": zero(), "unattributed": zero()}
    for row in rows:
        item = {field: safe_number(row[field], floating=field == "estimated_usd") for field in FIELDS}
        merge(by_key.setdefault(row["key_id"], zero()), item)
        model = row["model"]
        if model not in public_models:
            model = "other"
        merge(by_model.setdefault(model, zero()), item)
        merge(total, item)
        pool = {"openai_account": "openai", "claude_subscription_account": "claude"}.get(row["actual_source_kind"], "unattributed")
        merge(by_pool[pool], item)
    return by_key, by_model, total, by_pool


def _request_health_day():
    return {
        "requests": 0,
        "success": 0,
        "error_categories": {kind: 0 for kind in REQUEST_ERROR_KINDS},
        "error_layers": None,
        "signatures": {},
        "failure_statuses": {},
        "duration_total": 0.0,
        "duration_count": 0,
        "first_response_total": 0.0,
        "first_response_count": 0,
        "last_success_at": None,
        "last_failure_at": None,
    }


def _merge_signature_entry(target_map, sig_key, diag, count, status_code, model_name, key_id, last_seen, sample_msg):
    entry = target_map.get(sig_key)
    if entry is None:
        entry = {
            "signature": diag["signature"],
            "layer": diag["layer"],
            "severity": diag["severity"],
            "title": diag["title"],
            "meaning": diag["meaning"],
            "action": diag["action"],
            "count": 0,
            "status_codes": set(),
            "model_counts": {},
            "key_ids": set(),
            "last_seen_at": None,
            "sample_message": "",
        }
        target_map[sig_key] = entry
    entry["count"] += count
    if status_code is not None:
        entry["status_codes"].add(status_code)
    if model_name:
        entry["model_counts"][model_name] = entry["model_counts"].get(model_name, 0) + count
    if key_id:
        entry["key_ids"].add(key_id)
    if last_seen is not None and (entry["last_seen_at"] is None or last_seen >= entry["last_seen_at"]):
        entry["last_seen_at"] = last_seen
        if sample_msg:
            entry["sample_message"] = sample_msg


def _request_health_summary(day_states):
    requests = success = duration_count = first_response_count = 0
    duration_total = first_response_total = 0.0
    categories = {kind: 0 for kind in REQUEST_ERROR_KINDS}
    layers = {layer: 0 for layer in ERROR_LAYERS}
    statuses = {}
    sig_merged = {}
    last_success_at = last_failure_at = None
    for state in day_states:
        requests += state["requests"]
        success += state["success"]
        duration_total += state["duration_total"]
        duration_count += state["duration_count"]
        first_response_total += state["first_response_total"]
        first_response_count += state["first_response_count"]
        for kind, count in state["error_categories"].items():
            categories[kind] += count
        if state["error_layers"] is not None:
            for layer, count in state["error_layers"].items():
                layers[layer] += count
        else:
            for kind, count in state["error_categories"].items():
                layers[CATEGORY_FALLBACK_LAYER.get(kind, "upstream_openai")] += count
        for sig_key, entry in state["signatures"].items():
            target = sig_merged.get(sig_key)
            if target is None:
                target = {
                    "signature": entry["signature"],
                    "layer": entry["layer"],
                    "severity": entry["severity"],
                    "title": entry["title"],
                    "meaning": entry["meaning"],
                    "action": entry["action"],
                    "count": 0,
                    "status_codes": set(),
                    "model_counts": {},
                    "key_ids": set(),
                    "last_seen_at": None,
                    "sample_message": "",
                }
                sig_merged[sig_key] = target
            target["count"] += entry["count"]
            target["status_codes"].update(entry["status_codes"])
            for model_name, model_cnt in entry["model_counts"].items():
                target["model_counts"][model_name] = target["model_counts"].get(model_name, 0) + model_cnt
            target["key_ids"].update(entry["key_ids"])
            if entry["last_seen_at"] is not None and (
                target["last_seen_at"] is None or entry["last_seen_at"] >= target["last_seen_at"]
            ):
                target["last_seen_at"] = entry["last_seen_at"]
                if entry["sample_message"]:
                    target["sample_message"] = entry["sample_message"]
        for code, count in state["failure_statuses"].items():
            statuses[code] = statuses.get(code, 0) + count
        if state["last_success_at"] is not None:
            last_success_at = max(last_success_at or 0, state["last_success_at"])
        if state["last_failure_at"] is not None:
            last_failure_at = max(last_failure_at or 0, state["last_failure_at"])
    failed = max(0, requests - success)
    cancelled = min(failed, categories["client_cancelled"])
    system_failed = max(0, failed - cancelled)
    effective_denom = max(0, requests - cancelled)
    effective_rate = float(round(success * 100 / effective_denom, 2)) if effective_denom else (100.0 if requests else 0.0)
    ordered_sigs = sorted(
        sig_merged.values(),
        key=lambda item: (SEVERITY_RANK.get(item["severity"], 9), -item["count"], item["signature"]),
    )[:25]
    signatures_out = [
        {
            "signature": item["signature"],
            "layer": item["layer"],
            "severity": item["severity"],
            "title": item["title"],
            "meaning": item["meaning"],
            "action": item["action"],
            "count": item["count"],
            "status_codes": sorted(item["status_codes"])[:12],
            "top_models": [
                m for m, _ in sorted(item["model_counts"].items(), key=lambda pair: -pair[1])[:6]
            ],
            "affected_keys": len(item["key_ids"]),
            "last_seen_at": item["last_seen_at"],
            "sample_message": item["sample_message"][:310],
        }
        for item in ordered_sigs
    ]
    return {
        "requests": requests,
        "success": success,
        "failed": failed,
        "success_rate": float(round(success * 100 / requests, 2)) if requests else 0.0,
        "effective_success_rate": effective_rate,
        "client_cancelled_count": cancelled,
        "system_failed_count": system_failed,
        "error_categories": [{"kind": kind, "count": categories[kind]} for kind in REQUEST_ERROR_KINDS],
        "error_layers": [{"layer": layer, "count": layers[layer]} for layer in ERROR_LAYERS],
        "signatures": signatures_out,
        "failure_statuses": [{"status_code": code, "count": statuses[code]} for code in sorted(statuses)],
        "average_duration_ms": float(round(duration_total / duration_count, 2)) if duration_count else None,
        "average_first_response_ms": float(round(first_response_total / first_response_count, 2))
        if first_response_count else None,
        "last_success_at": last_success_at,
        "last_failure_at": last_failure_at,
    }


def collect_request_health(connection, today, end, public_models=frozenset()):
    """Aggregate retained request outcomes and 14d error signatures without exposing raw secrets."""
    include_duration = include_first_response = True
    while True:
        duration_total = ("MAX(COALESCE(duration_ms,0),0)" if include_duration else "0")
        duration_present = ("duration_ms IS NOT NULL" if include_duration else "0")
        first_total = ("MAX(COALESCE(first_response_ms,0),0)" if include_first_response else "0")
        first_present = ("first_response_ms IS NOT NULL" if include_first_response else "0")
        summary_sql = f"""
          SELECT CAST((created_at + 28800) / 86400 AS INTEGER) AS day_id,
            COUNT(*) AS requests,
            SUM(CASE WHEN status_code >= 200 AND status_code <= 299 THEN 1 ELSE 0 END) AS success,
            SUM(CASE WHEN status_code >= 200 AND status_code <= 299 AND {duration_present}
              THEN {duration_total} ELSE 0 END) AS duration_total,
            SUM(CASE WHEN status_code >= 200 AND status_code <= 299 AND {duration_present}
              THEN 1 ELSE 0 END) AS duration_count,
            SUM(CASE WHEN status_code >= 200 AND status_code <= 299 AND {first_present}
              THEN {first_total} ELSE 0 END) AS first_response_total,
            SUM(CASE WHEN status_code >= 200 AND status_code <= 299 AND {first_present}
              THEN 1 ELSE 0 END) AS first_response_count,
            MAX(CASE WHEN status_code >= 200 AND status_code <= 299 THEN created_at ELSE NULL END) AS last_success_at
          FROM request_logs WHERE created_at < ?
          GROUP BY day_id ORDER BY day_id
        """
        try:
            summary_rows = connection.execute(summary_sql, (end,)).fetchall()
            break
        except sqlite3.OperationalError as exc:
            if str(exc) == "no such table: request_logs":
                return None
            if include_duration and str(exc) == "no such column: duration_ms":
                include_duration = False
                continue
            if include_first_response and str(exc) == "no such column: first_response_ms":
                include_first_response = False
                continue
            raise

    failure_sql = """
      SELECT CAST((created_at + 28800) / 86400 AS INTEGER) AS day_id,
        request_error_category(status_code,error) AS category,
        status_code,COUNT(*) AS requests,MAX(created_at) AS last_failure_at
      FROM request_logs
      WHERE created_at < ? AND NOT (status_code >= 200 AND status_code <= 299)
      GROUP BY day_id,category,status_code ORDER BY day_id
    """
    failure_rows = connection.execute(failure_sql, (end,)).fetchall()
    states = {}
    for row in summary_rows:
        day_id = safe_number(row["day_id"])
        state = states.setdefault(day_id, _request_health_day())
        requests = safe_number(row["requests"])
        success = min(requests, safe_number(row["success"]))
        state["requests"] += requests
        state["success"] += success
        state["duration_total"] += safe_number(row["duration_total"], floating=True)
        state["duration_count"] += safe_number(row["duration_count"])
        state["first_response_total"] += safe_number(row["first_response_total"], floating=True)
        state["first_response_count"] += safe_number(row["first_response_count"])
        if row["last_success_at"] is not None:
            state["last_success_at"] = max(state["last_success_at"] or 0, safe_number(row["last_success_at"]))
    for row in failure_rows:
        day_id = safe_number(row["day_id"])
        state = states.setdefault(day_id, _request_health_day())
        count = safe_number(row["requests"])
        category = row["category"]
        if category not in state["error_categories"]:
            category = "unknown_error"
        state["error_categories"][category] += count
        status_code = row["status_code"]
        if type(status_code) is int and 100 <= status_code <= 599:
            state["failure_statuses"][status_code] = state["failure_statuses"].get(status_code, 0) + count
        if row["last_failure_at"] is not None:
            state["last_failure_at"] = max(state["last_failure_at"] or 0, safe_number(row["last_failure_at"]))

    local_epoch = int(today.timestamp()) + 8 * 3600
    today_id = local_epoch // 86400
    week_id = today_id - 6
    history_start_id = today_id - REQUEST_HEALTH_HISTORY_DAYS + 1
    history_start_ts = history_start_id * 86400 - 8 * 3600

    include_sig_model = include_sig_key = True
    while True:
        model_expr = "model" if include_sig_model else "NULL AS model"
        key_expr = "key_id" if include_sig_key else "NULL AS key_id"
        sig_sql = f"""
          SELECT CAST((created_at + 28800) / 86400 AS INTEGER) AS day_id,
                 status_code, error, {model_expr}, {key_expr}, created_at
          FROM request_logs
          WHERE created_at >= ? AND created_at < ?
            AND NOT (status_code >= 200 AND status_code <= 299)
          ORDER BY created_at ASC
        """
        try:
            sig_rows = connection.execute(sig_sql, (history_start_ts, end)).fetchall()
            break
        except sqlite3.OperationalError as exc:
            msg = str(exc)
            if include_sig_model and "no such column: model" in msg:
                include_sig_model = False
                continue
            if include_sig_key and "no such column: key_id" in msg:
                include_sig_key = False
                continue
            sig_rows = []
            break

    for row in sig_rows:
        day_id = safe_number(row["day_id"])
        if day_id < history_start_id or day_id > today_id:
            continue
        state = states.setdefault(day_id, _request_health_day())
        if state["error_layers"] is None:
            state["error_layers"] = {layer: 0 for layer in ERROR_LAYERS}
        code = row["status_code"] if type(row["status_code"]) is int and 100 <= row["status_code"] <= 599 else None
        diag = classify_failure(code, row["error"])
        state["error_layers"][diag["layer"]] += 1
        model_name = _safe_public_model(row["model"], public_models)
        raw_key = row["key_id"] if isinstance(row["key_id"], str) and row["key_id"] else None
        occurred_at = safe_number(row["created_at"])
        sample_msg = safe_failure_message(code, row["error"], diag)
        _merge_signature_entry(
            state["signatures"],
            diag["signature"],
            diag,
            1,
            code,
            model_name,
            raw_key,
            occurred_at,
            sample_msg,
        )

    empty = _request_health_day()
    days = []
    for day_id in range(history_start_id, today_id + 1):
        item = _request_health_summary([states.get(day_id, empty)])
        item["date"] = datetime.fromtimestamp(day_id * 86400, timezone.utc).date().isoformat()
        days.append(item)
    return {
        "today": _request_health_summary([states.get(today_id, empty)]),
        "week": _request_health_summary([state for day_id, state in states.items() if week_id <= day_id <= today_id]),
        "recorded": _request_health_summary(states.values()),
        "days": days,
        "history_days": REQUEST_HEALTH_HISTORY_DAYS,
    }


def collect_account_health(connection, today_ts, week_ts, end):
    """Per-account today/7d request volume and non-cancelled success rate."""
    try:
        rows = connection.execute("""
          SELECT account_id,
            SUM(CASE WHEN created_at >= ? THEN 1 ELSE 0 END) AS today_requests,
            SUM(CASE WHEN created_at >= ? AND status_code >= 200 AND status_code <= 299 THEN 1 ELSE 0 END) AS today_success,
            SUM(CASE WHEN created_at >= ? AND status_code = 499 THEN 1 ELSE 0 END) AS today_cancelled,
            COUNT(*) AS week_requests,
            MAX(created_at) AS last_used_at
          FROM request_logs
          WHERE created_at >= ? AND created_at < ? AND account_id IS NOT NULL
          GROUP BY account_id
        """, (today_ts, today_ts, today_ts, week_ts, end)).fetchall()
    except sqlite3.OperationalError:
        return {}
    result = {}
    for row in rows:
        today_req = safe_number(row["today_requests"])
        today_ok = safe_number(row["today_success"])
        today_cancel = safe_number(row["today_cancelled"])
        denom = max(0, today_req - today_cancel)
        rate = float(round(today_ok * 100 / denom, 1)) if denom else (100.0 if today_req else None)
        result[row["account_id"]] = {
            "today_requests": today_req,
            "today_success_rate": rate,
            "week_requests": safe_number(row["week_requests"]),
            "last_used_at": optional_timestamp(row["last_used_at"]),
        }
    return result


def collect_model_weather(connection, today_ts, week_ts, end, public_models, speed):
    """Real-time per-model health, overload signals, TTFT P50, and decode tok/s P50."""
    try:
        rows = connection.execute("""
          SELECT model,
            SUM(CASE WHEN created_at >= ? THEN 1 ELSE 0 END) AS today_requests,
            SUM(CASE WHEN created_at >= ? AND status_code >= 200 AND status_code <= 299 THEN 1 ELSE 0 END) AS today_success,
            SUM(CASE WHEN created_at >= ? AND status_code = 499 THEN 1 ELSE 0 END) AS today_cancelled,
            SUM(CASE WHEN created_at >= ? AND request_error_category(status_code, error) = 'upstream_overloaded' THEN 1 ELSE 0 END) AS today_overloads,
            COUNT(*) AS week_requests,
            SUM(CASE WHEN status_code >= 200 AND status_code <= 299 THEN 1 ELSE 0 END) AS week_success,
            SUM(CASE WHEN status_code = 499 THEN 1 ELSE 0 END) AS week_cancelled,
            SUM(CASE WHEN request_error_category(status_code, error) = 'upstream_overloaded' THEN 1 ELSE 0 END) AS week_overloads
          FROM request_logs
          WHERE created_at >= ? AND created_at < ? AND model IS NOT NULL
          GROUP BY model
        """, (today_ts, today_ts, today_ts, today_ts, week_ts, end)).fetchall()
    except sqlite3.OperationalError:
        return []

    by_model = {}
    for row in rows:
        raw_model = row["model"]
        if raw_model not in public_models:
            continue
        by_model[raw_model] = {
            "today_requests": safe_number(row["today_requests"]),
            "today_success": safe_number(row["today_success"]),
            "today_cancelled": safe_number(row["today_cancelled"]),
            "today_overloads": safe_number(row["today_overloads"]),
            "week_requests": safe_number(row["week_requests"]),
            "week_success": safe_number(row["week_success"]),
            "week_cancelled": safe_number(row["week_cancelled"]),
            "week_overloads": safe_number(row["week_overloads"]),
        }

    ttft_samples = {}
    try:
        ttft_rows = connection.execute("""
          SELECT model, first_response_ms
          FROM request_logs
          WHERE created_at >= ? AND created_at < ?
            AND status_code >= 200 AND status_code <= 299
            AND first_response_ms > 0 AND model IS NOT NULL
        """, (week_ts, end)).fetchall()
        for row in ttft_rows:
            m = row["model"]
            if m in public_models:
                ttft_samples.setdefault(m, []).append(float(safe_number(row["first_response_ms"])))
    except sqlite3.OperationalError:
        pass

    decode_map = {}
    if speed and speed.get("models"):
        for item in speed["models"]:
            decode_map[item["model"]] = item.get("decode_p50")

    weather = []
    for model_name, item in by_model.items():
        if model_name == "codex-auto-review":
            continue
        t_req = item["today_requests"]
        t_denom = max(0, t_req - item["today_cancelled"])
        t_rate = float(round(item["today_success"] * 100 / t_denom, 1)) if t_denom else (100.0 if t_req else None)
        w_req = item["week_requests"]
        w_denom = max(0, w_req - item["week_cancelled"])
        w_rate = float(round(item["week_success"] * 100 / w_denom, 1)) if w_denom else (100.0 if w_req else None)
        t_overload = item["today_overloads"]
        if w_req == 0:
            status = "idle"
        elif t_overload >= 5 or (t_req >= 5 and t_rate is not None and t_rate < 75.0):
            status = "degraded"
        elif t_overload >= 1 or (t_req >= 5 and t_rate is not None and t_rate < 92.0):
            status = "busy"
        else:
            status = "smooth"
        weather.append({
            "model": model_name,
            "today_requests": t_req,
            "week_requests": w_req,
            "today_success_rate": t_rate,
            "week_effective_success_rate": w_rate,
            "today_overload_count": t_overload,
            "week_overload_count": item["week_overloads"],
            "decode_p50": decode_map.get(model_name),
            "ttft_p50_ms": _percentile(ttft_samples.get(model_name, []), 0.5),
            "status": status,
        })
    weather.sort(key=lambda item: (-item["today_requests"], -item["week_requests"], item["model"]))
    return weather[:50]


def collect(config, now=None):
    now = int(time.time()) if now is None else now
    today = datetime.fromtimestamp(now, TZ).replace(hour=0, minute=0, second=0, microsecond=0)
    today_ts = int(today.timestamp())
    week_ts = int((today - timedelta(days=6)).timestamp())
    history_start_ts = int((today - timedelta(days=REQUEST_HEALTH_HISTORY_DAYS - 1)).timestamp())
    # Day-aligned boundaries include only elapsed data; end is next local midnight.
    end = int((today + timedelta(days=1)).timestamp())
    secret = config["id_secret"]
    if not re.fullmatch(r"[a-f0-9]{64}", secret):
        raise ValueError("invalid id secret")
    with source_connection(Path(config["source_db"])) as connection:
        raw_accounts = connection.execute("""
          SELECT a.id,a.status,a.label,a.sort,a.group_name,s.account_plan_type,s.plan_type,s.expires_at,s.renews_at,
            c.primary_window_tokens,c.secondary_window_tokens,
            CASE WHEN p.account_id IS NULL THEN 0 ELSE 1 END AS proxy_present,
            CASE WHEN p.enabled <> 0 THEN 1 ELSE 0 END AS proxy_enabled,
            u.used_percent,u.window_minutes,u.resets_at,
            u.secondary_used_percent,u.secondary_window_minutes,u.secondary_resets_at,u.captured_at,u.credits_json
          FROM accounts a LEFT JOIN account_subscriptions s ON s.account_id=a.id
          LEFT JOIN account_quota_capacity_overrides c ON c.account_id=a.id
          LEFT JOIN account_proxy_settings p ON p.account_id=a.id
          LEFT JOIN usage_snapshots u ON u.id=(SELECT x.id FROM usage_snapshots x
            WHERE x.account_id=a.id ORDER BY x.captured_at DESC,x.id DESC LIMIT 1)
          ORDER BY a.sort,a.updated_at DESC,a.id
        """).fetchall()
        try:
            raw_claude_accounts = connection.execute("""
              SELECT id,email,subscription_type,status,sort,updated_at
              FROM claude_subscription_accounts ORDER BY sort,updated_at DESC,id
            """).fetchall()
        except sqlite3.OperationalError as exc:
            if str(exc) != "no such table: claude_subscription_accounts":
                raise
            raw_claude_accounts = []
        try:
            raw_claude_usage = connection.execute("""
              SELECT account_id,five_hour_used_percent,five_hour_resets_at,
                     seven_day_used_percent,seven_day_resets_at,captured_at,
                     last_attempt_at,next_attempt_at,last_error
              FROM claude_subscription_usage
              ORDER BY captured_at DESC,last_attempt_at DESC
            """).fetchall()
        except sqlite3.OperationalError as exc:
            if str(exc) != "no such table: claude_subscription_usage":
                raise
            raw_claude_usage = []
        key_sql = """
          SELECT k.id,k.name,k.status,k.created_at,k.last_used_at,k.account_group_filter,
            COALESCE(p.protocol_type,'openai_compat') AS protocol_type,
            COALESCE(k.rotation_strategy,'account_rotation') AS rotation_strategy,
            COALESCE(p.default_model,k.model_slug) AS model_slug,q.quota_limit_tokens,
            k.upstream_provider AS upstream_provider
          FROM api_keys k LEFT JOIN api_key_profiles p ON p.key_id=k.id
          LEFT JOIN api_key_quota_limits q ON q.key_id=k.id ORDER BY k.created_at DESC,k.id
        """
        try:
            raw_keys = connection.execute(key_sql).fetchall()
        except sqlite3.OperationalError as exc:
            if str(exc) != "no such column: k.upstream_provider":
                raise
            raw_keys = connection.execute(key_sql.replace("k.upstream_provider", "'unknown'")).fetchall()

        catalog, unlisted = collect_catalog(connection)
        public_models = frozenset(item["model"] for item in catalog)
        periods = {
            "today": aggregate(connection, today_ts, end, public_models),
            "week": aggregate(connection, week_ts, end, public_models),
            "recorded": aggregate(connection, None, end, public_models),
        }
        request_health = collect_request_health(connection, today, end, public_models)
        key_weekly_cents = collect_key_weekly_cents(connection, end)
        key_next_release = collect_key_next_release(connection, end, key_weekly_cents)
        account_weekly_cents = collect_account_weekly_cents(connection, end)
        account_health = collect_account_health(connection, today_ts, week_ts, end)
        speed = collect_speed(connection, end, public_models)
        model_weather = collect_model_weather(connection, today_ts, week_ts, end, public_models, speed)

        account_labels = {}
        for row in raw_accounts:
            pid = public_id("acct", row["id"], secret)
            account_labels[row["id"]] = safe_label(config.get("account_aliases", {}).get(row["id"]), "账号 " + pid[-6:])
        key_labels = {}
        for row in raw_keys:
            pid = public_id("key", row["id"], secret)
            key_labels[row["id"]] = public_text(row["name"], "Key " + pid[-6:])

        recent_failures = collect_recent_failures(
            connection, history_start_ts, end, secret, account_labels, key_labels, public_models
        )

    accounts = []
    for row in raw_accounts:
        pid = public_id("acct", row["id"], secret)
        plan = "unknown"
        for candidate in (row["account_plan_type"], row["plan_type"]):
            normalized = str(candidate or "").lower().strip()
            if normalized in {"pro", "plus", "team", "business", "enterprise", "free"}:
                plan = normalized
                break
        capacity_primary = positive_int(row["primary_window_tokens"])
        capacity_secondary = positive_int(row["secondary_window_tokens"])
        ah = account_health.get(row["id"], {})
        accounts.append({"id": pid, "label": account_labels[row["id"]],
                         "group_name": group_identity(row["group_name"], secret)[1],
                         "plan": plan, "status": safe_status(row["status"]),
                         "primary": quota_window(row), "secondary": quota_window(row, "secondary_"),
                         "captured_at": optional_timestamp(row["captured_at"]),
                         "email": email_from_label(row["label"]),
                         "subscription_expires_at": timestamp(row["expires_at"]),
                         "subscription_renews_at": timestamp(row["renews_at"]),
                         "sort_order": max(0, row["sort"]) if type(row["sort"]) is int else None,
                         "proxy_state": "inherited" if not row["proxy_present"] else "configured" if row["proxy_enabled"] else "disabled",
                         "capacity_override": bool(capacity_primary or capacity_secondary),
                         "capacity_primary_tokens": capacity_primary, "capacity_secondary_tokens": capacity_secondary,
                         "weekly_used_cents": account_weekly_cents.get(row["id"], 0),
                         "weekly_limit_cents": None,
                         "weekly_remaining_cents": None,
                         "weekly_reset_at": optional_timestamp(row["resets_at"]),
                         "today_requests": ah.get("today_requests", 0),
                         "today_success_rate": ah.get("today_success_rate"),
                         "week_requests": ah.get("week_requests", 0),
                         "last_used_at": ah.get("last_used_at"),
                         **credits_details(row["credits_json"], optional_timestamp(row["captured_at"]))})
    claude_usage_by_account = {}
    for usage_row in raw_claude_usage:
        claude_usage_by_account.setdefault(usage_row["account_id"], usage_row)
    claude_accounts = []
    for row in raw_claude_accounts:
        pid = public_id("acct", row["id"], secret)
        usage = claude_usage_by_account.get(row["id"])
        captured_at = timestamp(usage["captured_at"]) if usage else None
        claude_accounts.append({
            "id": pid,
            "label": safe_label(config.get("account_aliases", {}).get(row["id"]), "Claude " + pid[-6:]),
            "email": email_from_label(row["email"]),
            "plan": claude_plan(row["subscription_type"]),
            "status": claude_status(row["status"]),
            "sort_order": max(0, row["sort"]) if type(row["sort"]) is int else None,
            "updated_at": optional_timestamp(row["updated_at"]),
            "five_hour": claude_quota_window(usage["five_hour_used_percent"], usage["five_hour_resets_at"], 300)
                         if usage and captured_at else claude_quota_window(None, None, 300),
            "seven_day": claude_quota_window(usage["seven_day_used_percent"], usage["seven_day_resets_at"], 10080)
                         if usage and captured_at else claude_quota_window(None, None, 10080),
            "captured_at": captured_at,
            "last_attempt_at": timestamp(usage["last_attempt_at"]) if usage else None,
            "next_attempt_at": timestamp(usage["next_attempt_at"]) if usage else None,
            "last_error": claude_usage_error(usage["last_error"]) if usage else None,
        })
    keys = []
    for row in raw_keys:
        key_id = row["id"]
        pid = public_id("key", key_id, secret)
        release = key_next_release.get(key_id)
        keys.append({"id": pid, "label": key_labels[key_id],
                     "group_name": group_identity(row["account_group_filter"], secret)[1],
                     "status": safe_status(row["status"]),
                     "last_used_at": optional_timestamp(row["last_used_at"]),
                     "usage": {name: data[0].get(key_id, zero()) for name, data in periods.items()},
                     "weekly_used_cents": key_weekly_cents.get(key_id, 0),
                     "weekly_limit_cents": 24000,
                     "weekly_remaining_cents": max(0, 24000 - key_weekly_cents.get(key_id, 0)),
                     "next_release_at": release[0] if release else None,
                     "next_release_cents": release[1] if release else 0,
                     **key_metadata(row, public_models)})
    models = sorted(periods["week"][1].items(), key=lambda pair: pair[1]["total_tokens"], reverse=True)
    if len(models) > 50:
        tail = zero()
        for _, item in models[49:]:
            merge(tail, item)
        models = models[:49] + [("other-models", tail)]
    snapshot = {"schema_version": 1, "generated_at": now, "timezone": "Asia/Shanghai",
                "accounts": accounts, "claude_accounts": claude_accounts,
                "keys": keys, "model_catalog": catalog, "catalog_unlisted_count": unlisted,
                "key_groups": collect_key_groups(raw_keys, periods, secret),
                "totals": {name: data[2] for name, data in periods.items()},
                "pool_usage": {pool: {name: data[3][pool] for name, data in periods.items()}
                               for pool in ("openai", "claude", "unattributed")},
                "models_week": [{"model": model, "usage": item} for model, item in models],
                "model_weather": model_weather,
                "request_health": request_health, "recent_failures": recent_failures,
                "speed": speed}
    raw_origins = config.get("terminal_origins")
    if isinstance(raw_origins, dict) and isinstance(raw_origins.get("public"), dict):
        pub = raw_origins["public"]
        origins_obj = {
            "public": {"label": str(pub.get("label", "")).strip(), "url": str(pub.get("url", "")).strip().rstrip("/")},
        }
        if isinstance(raw_origins.get("intranet"), dict):
            lan = raw_origins["intranet"]
            origins_obj["intranet"] = {
                "label": str(lan.get("label", "")).strip(),
                "url": str(lan.get("url", "")).strip().rstrip("/"),
            }
        snapshot["terminal_origins"] = origins_obj
    return Snapshot.model_validate(snapshot).model_dump()


def write_snapshot(path, snapshot):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    encoded = json.dumps(snapshot, ensure_ascii=False, allow_nan=False).encode("utf-8")
    if len(encoded) > 4 * 1024 * 1024:
        raise ValueError("snapshot too large")
    fd, temporary = tempfile.mkstemp(prefix="snapshot-", suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def validate_output_path(output, source):
    target, database = Path(output).resolve(), Path(source).resolve(strict=True)
    if target in {database, Path(str(database) + "-wal"), Path(str(database) + "-shm")}:
        raise ValueError("output must not replace a source database file")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-db", type=Path, help="Container-local source path; does not modify private configuration")
    parser.add_argument("--once", action="store_true")
    args = parser.parse_args()
    if args.once:
        handlers = [logging.StreamHandler()]
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        handlers = [RotatingFileHandler(args.output.parent / "collector.log", maxBytes=1024*1024,
                                        backupCount=3, encoding="utf-8")]
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s", handlers=handlers)
    while True:
        started = time.monotonic()
        try:
            config = json.loads(args.config.read_text(encoding="utf-8"))
            if args.source_db is not None:
                config["source_db"] = str(args.source_db)
            validate_output_path(args.output, config["source_db"])
            write_snapshot(args.output, collect(config))
            LOG.info("snapshot_updated")
        except Exception as error:
            # Keep last good snapshot; exception messages may include source data.
            LOG.error("snapshot_update_failed type=%s", type(error).__name__)
            if args.once:
                raise SystemExit(1) from None
        if args.once:
            break
        time.sleep(max(5, 60 - (time.monotonic() - started)))


if __name__ == "__main__":
    main()
