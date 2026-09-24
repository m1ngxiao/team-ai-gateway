"""Claude account and provider statistics use only approved, read-only fields."""

import json
import sqlite3

import pytest

from dashboard.collector import FIELDS, collect, public_id, source_connection
from dashboard.models import Snapshot


def test_old_database_and_snapshot_wait_for_provider_data(sample):
    data = collect(sample["config"], sample["now"])
    assert data["claude_accounts"] == []
    assert all(key["upstream_provider"] == "unknown" for key in data["keys"])
    for period in ("today", "week", "recorded"):
        assert data["pool_usage"]["unattributed"][period] == data["totals"][period]
        assert all(value == 0 for value in data["pool_usage"]["claude"][period].values())
    del data["claude_accounts"]
    del data["pool_usage"]
    for key in data["keys"]:
        del key["upstream_provider"]
    parsed = Snapshot.model_validate(data)
    assert parsed.claude_accounts == [] and parsed.pool_usage is None
    assert parsed.keys[0].upstream_provider == "unknown"


def add_claude_schema(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.executescript("""
          ALTER TABLE api_keys ADD COLUMN upstream_provider TEXT NOT NULL DEFAULT 'openai';
          ALTER TABLE request_token_stats ADD COLUMN actual_source_kind TEXT;
          ALTER TABLE request_token_stat_hourly_rollups ADD COLUMN actual_source_kind TEXT;
          CREATE TABLE claude_subscription_accounts (
            id TEXT PRIMARY KEY,label TEXT,email TEXT,subscription_type TEXT,status TEXT,
            sort INT,updated_at INT,access_token TEXT,refresh_token TEXT,scopes TEXT,last_error TEXT
          );
        """)
        db.execute("INSERT INTO claude_subscription_accounts VALUES(?,?,?,?,?,?,?,?,?,?,?)",
                   ("internal-claude", sample["canary"], "member@example.com", "claude_pro", "active", 2,
                    sample["now"], sample["canary"], sample["canary"], sample["canary"], sample["canary"]))
        # Today's OpenAI request is still OpenAI even after its Key changes provider.
        db.execute("UPDATE api_keys SET upstream_provider='claude' WHERE id='internal-key-a'")
        db.execute("UPDATE request_token_stats SET actual_source_kind='openai_account' WHERE request_log_id IN (1,2)")
        db.execute("UPDATE request_token_stat_hourly_rollups SET actual_source_kind='openai_account'")
        # A deleted test Key still contributes to the true Claude account pool.
        db.execute("INSERT INTO request_token_stats(request_log_id,key_id,model,usage_included,created_at,"
                   "input_tokens,cached_input_tokens,output_tokens,total_tokens,estimated_cost_usd,actual_source_kind) "
                   "VALUES(4,'deleted-claude-key','claude-sonnet-5',1,?,40,0,10,50,.05,'claude_subscription_account')",
                   (sample["day"] + 3,))
        db.execute("INSERT INTO models VALUES('model-claude','claude-sonnet-5','Claude Sonnet 5','Claude model','custom',1,'list',1,'passthrough',1,'')")
        db.execute("INSERT INTO model_routes VALUES('model-claude',1,'account_pool','claude','claude-sonnet-5')")


def test_claude_account_is_separate_and_actual_pool_usage_reconciles(sample):
    add_claude_schema(sample)
    source_before = sample["db"].read_bytes()
    data = collect(sample["config"], sample["now"])
    assert sample["db"].read_bytes() == source_before
    assert len(data["accounts"]) == len(data["claude_accounts"]) == 1
    claude = data["claude_accounts"][0]
    assert claude == {"id": public_id("acct", "internal-claude", sample["config"]["id_secret"]),
                      "label": "Claude " + public_id("acct", "internal-claude", sample["config"]["id_secret"])[-6:],
                      "email": "member@example.com", "plan": "pro", "status": "enabled", "sort_order": 2,
                      "updated_at": sample["now"],
                      "five_hour": {"minutes": 300, "remaining_percent": None, "resets_at": None},
                      "seven_day": {"minutes": 10080, "remaining_percent": None, "resets_at": None},
                      "captured_at": None, "last_attempt_at": None, "next_attempt_at": None, "last_error": None}
    key_id = public_id("key", "internal-key-a", sample["config"]["id_secret"])
    assert next(key for key in data["keys"] if key["id"] == key_id)["upstream_provider"] == "claude"
    for period in ("today", "week", "recorded"):
        for field in FIELDS:
            assert sum(data["pool_usage"][pool][period][field] for pool in ("openai", "claude", "unattributed")) == pytest.approx(data["totals"][period][field])
    assert data["pool_usage"]["openai"]["today"]["total_tokens"] == 100
    assert data["pool_usage"]["claude"]["today"]["total_tokens"] == 50
    assert data["pool_usage"]["claude"]["today"]["requests"] == 1
    assert data["pool_usage"]["unattributed"]["recorded"]["total_tokens"] == 600
    assert any(item["model"] == "claude-sonnet-5" for item in data["models_week"])
    assert any(item["model"] == "claude-sonnet-5" for item in data["model_catalog"])
    assert sample["canary"] not in json.dumps(data)
    assert "deleted-claude-key" not in json.dumps(data)


def test_claude_login_state_and_private_columns_are_not_exported(sample):
    add_claude_schema(sample)
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE claude_subscription_accounts SET status='needs_login',subscription_type='claude_max_20x' WHERE id='internal-claude'")
    account = collect(sample["config"], sample["now"])["claude_accounts"][0]
    assert account["status"] == "needs_login" and account["plan"] == "max"
    with source_connection(sample["db"]) as db:
        for column in ("label", "access_token", "refresh_token", "scopes", "last_error"):
            with pytest.raises(sqlite3.DatabaseError):
                db.execute(f"SELECT {column} FROM claude_subscription_accounts").fetchall()


def test_actual_pool_usage_ignores_current_key_provider(sample):
    add_claude_schema(sample)
    baseline = collect(sample["config"], sample["now"])["pool_usage"]
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET upstream_provider='openai'")
        db.execute("DELETE FROM api_keys WHERE id='internal-key-a'")
    after = collect(sample["config"], sample["now"])
    assert after["pool_usage"] == baseline
    assert after["pool_usage"]["claude"]["today"]["total_tokens"] == 50


def test_claude_quota_snapshot_is_separate_from_gateway_token_usage(sample):
    add_claude_schema(sample)
    with sqlite3.connect(sample["db"]) as db:
        db.executescript("""
          CREATE TABLE claude_subscription_usage (
            account_id TEXT PRIMARY KEY,five_hour_used_percent REAL,five_hour_resets_at INT,
            seven_day_used_percent REAL,seven_day_resets_at INT,captured_at INT,
            last_attempt_at INT,next_attempt_at INT,last_error TEXT,private_response TEXT
          );
        """)
        db.execute("INSERT INTO claude_subscription_usage VALUES(?,?,?,?,?,?,?,?,?,?)",
                   ("internal-claude", 24.5, sample["now"] + 3600, 70.0, sample["now"] + 604800,
                    sample["now"], sample["now"], sample["now"] + 600, "HTTP 429", sample["canary"]))
    source_before = sample["db"].read_bytes()
    data = collect(sample["config"], sample["now"])
    assert sample["db"].read_bytes() == source_before
    account = data["claude_accounts"][0]
    assert account["five_hour"] == {"minutes": 300, "remaining_percent": 75.5, "resets_at": sample["now"] + 3600}
    assert account["seven_day"] == {"minutes": 10080, "remaining_percent": 30.0, "resets_at": sample["now"] + 604800}
    assert account["captured_at"] == account["last_attempt_at"] == sample["now"]
    assert account["next_attempt_at"] == sample["now"] + 600
    assert account["last_error"] == "rate_limited"
    assert data["pool_usage"]["claude"]["today"]["total_tokens"] == 50
    assert sample["canary"] not in json.dumps(data)
    with source_connection(sample["db"]) as db:
        with pytest.raises(sqlite3.DatabaseError):
            db.execute("SELECT private_response FROM claude_subscription_usage").fetchall()


def test_claude_quota_invalid_values_do_not_become_invented_remaining(sample):
    add_claude_schema(sample)
    with sqlite3.connect(sample["db"]) as db:
        db.executescript("""
          CREATE TABLE claude_subscription_usage (
            account_id TEXT PRIMARY KEY,five_hour_used_percent REAL,five_hour_resets_at INT,
            seven_day_used_percent REAL,seven_day_resets_at INT,captured_at INT,
            last_attempt_at INT,next_attempt_at INT,last_error TEXT
          );
        """)
        db.execute("INSERT INTO claude_subscription_usage VALUES(?,?,?,?,?,?,?,?,?)",
                   ("internal-claude", 123.4, None, -2.0, None, sample["now"], sample["now"], None, sample["canary"]))
    account = collect(sample["config"], sample["now"])["claude_accounts"][0]
    assert account["five_hour"]["remaining_percent"] is None
    assert account["seven_day"]["remaining_percent"] is None
    assert account["last_error"] == "query_failed"
    assert sample["canary"] not in json.dumps(account)
