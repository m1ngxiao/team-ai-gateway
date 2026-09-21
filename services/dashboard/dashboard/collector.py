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
from .catalog import CATALOG_COLUMNS, PUBLIC_MODELS, collect_catalog, key_metadata, public_text

LOG = logging.getLogger("collector")
TZ = timezone(timedelta(hours=8))
FIELDS = ("requests", "input_tokens", "cached_tokens", "output_tokens", "total_tokens", "estimated_usd")
ALLOWED_COLUMNS = {
    **CATALOG_COLUMNS,
    "accounts": {"id", "status", "created_at", "updated_at", "label", "sort", "group_name"},
    "account_subscriptions": {"account_id", "account_plan_type", "plan_type", "expires_at", "renews_at"},
    "account_quota_capacity_overrides": {"account_id", "primary_window_tokens", "secondary_window_tokens"},
    "account_proxy_settings": {"account_id", "enabled"},
    "usage_snapshots": {"id", "account_id", "used_percent", "window_minutes", "resets_at",
                        "secondary_used_percent", "secondary_window_minutes", "secondary_resets_at", "captured_at", "credits_json"},
    "api_keys": {"id", "name", "status", "created_at", "last_used_at", "rotation_strategy", "model_slug", "account_group_filter"},
    "api_key_profiles": {"key_id", "protocol_type", "default_model"},
    "api_key_quota_limits": {"key_id", "quota_limit_tokens"},
    "request_token_stats": {"request_log_id", "key_id", "model", "usage_included", "created_at",
                            "input_tokens", "cached_input_tokens", "output_tokens", "total_tokens", "estimated_cost_usd"},
    "request_token_stat_hourly_rollups": {"key_id", "model", "bucket_start", "bucket_end", "request_count",
                                          "input_tokens", "cached_input_tokens", "output_tokens", "total_tokens", "estimated_cost_usd"},
    "request_token_stat_rollups": {"key_id", "model", "source_rows", "input_tokens",
                                    "cached_input_tokens", "output_tokens", "total_tokens", "estimated_cost_usd"},
}


def read_authorizer(action, table, column, database, trigger):
    if action == sqlite3.SQLITE_READ:
        return sqlite3.SQLITE_OK if database == "main" and column in ALLOWED_COLUMNS.get(table, set()) else sqlite3.SQLITE_DENY
    if action in {sqlite3.SQLITE_SELECT, sqlite3.SQLITE_TRANSACTION}:
        return sqlite3.SQLITE_OK
    if action == sqlite3.SQLITE_FUNCTION:
        return sqlite3.SQLITE_OK if column.lower() in {"sum", "count", "coalesce", "max"} else sqlite3.SQLITE_DENY
    return sqlite3.SQLITE_DENY


@contextmanager
def source_connection(path: Path):
    path = path.resolve(strict=True)
    connection = sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=2)
    try:
        connection.row_factory = sqlite3.Row
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

    for period_name, (by_key, _, _) in periods.items():
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


def aggregate(connection, start: int | None, end: int):
    """raw + disjoint hourly archives; undated legacy data is all-time only."""
    raw_filter = "t.created_at < ?" if start is None else "t.created_at >= ? AND t.created_at < ?"
    hourly_filter = "h.bucket_end <= ?" if start is None else "h.bucket_start >= ? AND h.bucket_end <= ?"
    args = [end, end] if start is None else [start, end, start, end]
    sql = f"""
      SELECT t.key_id, t.model,
        COUNT(t.request_log_id) AS requests,
        SUM(CASE WHEN t.usage_included=1 THEN MAX(COALESCE(t.input_tokens,0),0) ELSE 0 END) AS input_tokens,
        SUM(CASE WHEN t.usage_included=1 THEN MAX(COALESCE(t.cached_input_tokens,0),0) ELSE 0 END) AS cached_tokens,
        SUM(CASE WHEN t.usage_included=1 THEN MAX(COALESCE(t.output_tokens,0),0) ELSE 0 END) AS output_tokens,
        SUM(CASE WHEN t.usage_included<>1 THEN 0 ELSE MAX(COALESCE(t.total_tokens,
          COALESCE(t.input_tokens,0)-COALESCE(t.cached_input_tokens,0)+COALESCE(t.output_tokens,0)),0) END) AS total_tokens,
        SUM(CASE WHEN t.usage_included=1 THEN MAX(COALESCE(t.estimated_cost_usd,0),0) ELSE 0 END) AS estimated_usd
      FROM request_token_stats t WHERE {raw_filter} GROUP BY t.key_id,t.model
      UNION ALL
      SELECT h.key_id,h.model,SUM(h.request_count),SUM(h.input_tokens),SUM(h.cached_input_tokens),
        SUM(h.output_tokens),SUM(h.total_tokens),SUM(h.estimated_cost_usd)
      FROM request_token_stat_hourly_rollups h WHERE {hourly_filter} GROUP BY h.key_id,h.model
    """
    if start is None:
        sql += """ UNION ALL
          SELECT l.key_id,l.model,SUM(l.source_rows),SUM(l.input_tokens),SUM(l.cached_input_tokens),
            SUM(l.output_tokens),SUM(l.total_tokens),SUM(l.estimated_cost_usd)
          FROM request_token_stat_rollups l GROUP BY l.key_id,l.model"""
    by_key, by_model, total = {}, {}, zero()
    for row in connection.execute(sql, args):
        item = {field: safe_number(row[field], floating=field == "estimated_usd") for field in FIELDS}
        merge(by_key.setdefault(row["key_id"], zero()), item)
        model = row["model"]
        # Models are untrusted user input. Never echo an unapproved model string.
        if model not in PUBLIC_MODELS:
            model = "other"
        merge(by_model.setdefault(model, zero()), item)
        merge(total, item)
    return by_key, by_model, total


def collect(config, now=None):
    now = int(time.time()) if now is None else now
    today = datetime.fromtimestamp(now, TZ).replace(hour=0, minute=0, second=0, microsecond=0)
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
        raw_keys = connection.execute("""
          SELECT k.id,k.name,k.status,k.created_at,k.last_used_at,k.account_group_filter,
            COALESCE(p.protocol_type,'openai_compat') AS protocol_type,
            COALESCE(k.rotation_strategy,'account_rotation') AS rotation_strategy,
            COALESCE(p.default_model,k.model_slug) AS model_slug,q.quota_limit_tokens
          FROM api_keys k LEFT JOIN api_key_profiles p ON p.key_id=k.id
          LEFT JOIN api_key_quota_limits q ON q.key_id=k.id ORDER BY k.created_at DESC,k.id
        """).fetchall()
        catalog, unlisted = collect_catalog(connection)
        periods = {
            "today": aggregate(connection, int(today.timestamp()), end),
            "week": aggregate(connection, int((today - timedelta(days=6)).timestamp()), end),
            "recorded": aggregate(connection, None, end),
        }
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
        accounts.append({"id": pid, "label": safe_label(config.get("account_aliases", {}).get(row["id"]), "账号 " + pid[-6:]),
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
                         **credits_details(row["credits_json"], optional_timestamp(row["captured_at"]))})
    # Only current keys belong in the list. Deleted/unattributed usage remains
    # in the independently aggregated team totals and model usage above.
    keys = []
    for row in raw_keys:
        key_id = row["id"]
        pid = public_id("key", key_id, secret)
        # Display the administrator's name, never credential-looking content.
        # Empty/invalid names retain the existing stable, non-secret code.
        keys.append({"id": pid, "label": public_text(row["name"], "Key " + pid[-6:]),
                     "group_name": group_identity(row["account_group_filter"], secret)[1],
                     "status": safe_status(row["status"]),
                     "last_used_at": optional_timestamp(row["last_used_at"]),
                     "usage": {name: data[0].get(key_id, zero()) for name, data in periods.items()}, **key_metadata(row)})
    models = sorted(periods["week"][1].items(), key=lambda pair: pair[1]["total_tokens"], reverse=True)
    # Keep the top 49 plus one explicit 'other' bucket rather than silently losing usage.
    if len(models) > 50:
        tail = zero()
        for _, item in models[49:]:
            merge(tail, item)
        models = models[:49] + [("other-models", tail)]
    snapshot = {"schema_version": 1, "generated_at": now, "timezone": "Asia/Shanghai",
                "accounts": accounts, "keys": keys, "model_catalog": catalog, "catalog_unlisted_count": unlisted,
                "key_groups": collect_key_groups(raw_keys, periods, secret),
                "totals": {name: data[2] for name, data in periods.items()},
                "models_week": [{"model": model, "usage": item} for model, item in models]}
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
