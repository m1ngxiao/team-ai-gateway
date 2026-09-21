import json
import sqlite3
import time
from pathlib import Path

import pytest

from dashboard.collector import TZ, collect, write_snapshot
from dashboard.security import password_hash


@pytest.fixture
def sample(tmp_path):
    from datetime import datetime, timedelta
    now = int(time.time())
    day = int(datetime.fromtimestamp(now, TZ).replace(hour=0, minute=0, second=0, microsecond=0).timestamp())
    database = tmp_path / "manager.db"
    canary = "SECRET-TOKEN-NEVER-EXPORT: confidential@example.com"
    with sqlite3.connect(database) as db:
        db.executescript("""
          CREATE TABLE accounts(id TEXT PRIMARY KEY,status TEXT,created_at INT,label TEXT,sort INT DEFAULT 0,updated_at INT,group_name TEXT);
          CREATE TABLE account_subscriptions(account_id TEXT PRIMARY KEY,account_plan_type TEXT,plan_type TEXT,expires_at INT,renews_at INT);
          CREATE TABLE account_quota_capacity_overrides(account_id TEXT PRIMARY KEY,primary_window_tokens INT,secondary_window_tokens INT);
          CREATE TABLE account_proxy_settings(account_id TEXT PRIMARY KEY,enabled INT,proxy_url TEXT);
          CREATE TABLE usage_snapshots(id INTEGER PRIMARY KEY,account_id TEXT,used_percent REAL,window_minutes INT,
            resets_at INT,secondary_used_percent REAL,secondary_window_minutes INT,secondary_resets_at INT,captured_at INT,credits_json TEXT);
          CREATE TABLE api_keys(id TEXT PRIMARY KEY,status TEXT,created_at INT,last_used_at INT,name TEXT,key_hash TEXT,static_headers_json TEXT,rotation_strategy TEXT,model_slug TEXT,account_group_filter TEXT);
          CREATE TABLE api_key_profiles(key_id TEXT PRIMARY KEY,protocol_type TEXT,default_model TEXT,upstream_base_url TEXT);
          CREATE TABLE api_key_quota_limits(key_id TEXT PRIMARY KEY,quota_limit_tokens INT);
          CREATE TABLE api_key_secrets(key_id TEXT,key_value TEXT);
          CREATE TABLE tokens(account_id TEXT,access_token TEXT,refresh_token TEXT);
          CREATE TABLE request_logs(id INTEGER PRIMARY KEY,error TEXT,upstream_url TEXT);
          CREATE TABLE request_token_stats(request_log_id INT UNIQUE,key_id TEXT,model TEXT,usage_included INT,created_at INT,
            input_tokens INT,cached_input_tokens INT,output_tokens INT,total_tokens INT,estimated_cost_usd REAL);
          CREATE TABLE request_token_stat_hourly_rollups(key_id TEXT,model TEXT,bucket_start INT,bucket_end INT,request_count INT,
            input_tokens INT,cached_input_tokens INT,output_tokens INT,total_tokens INT,estimated_cost_usd REAL);
          CREATE TABLE request_token_stat_rollups(key_id TEXT,model TEXT,source_rows INT,input_tokens INT,cached_input_tokens INT,
            output_tokens INT,total_tokens INT,estimated_cost_usd REAL);
          CREATE TABLE models(id TEXT PRIMARY KEY,slug TEXT,display_name TEXT,description TEXT,origin TEXT,enabled INT,
            visibility TEXT,supported_in_api INT,instructions_mode TEXT,sort_order INT,instructions_text TEXT);
          CREATE TABLE model_prices(model_id TEXT PRIMARY KEY,price_status TEXT,input_microusd_per_1m INT,
            cached_input_microusd_per_1m INT,cache_write_microusd_per_1m INT,output_microusd_per_1m INT,price_source TEXT);
          CREATE TABLE model_routes(model_id TEXT,enabled INT,source_kind TEXT,source_id TEXT,upstream_model TEXT);
        """)
        db.execute("INSERT INTO accounts(id,status,created_at,label) VALUES(?,?,?,?)", ("internal-account", "active", day - 86400, canary))
        db.execute("INSERT INTO account_subscriptions(account_id,account_plan_type,plan_type) VALUES('internal-account','pro','unknown')")
        db.execute("INSERT INTO usage_snapshots VALUES(1,'internal-account',25,10080,?,NULL,NULL,NULL,?,?)", (now+3600, now, canary))
        key_names = {"internal-key-a": "测试 A / Codex CLI", "internal-key-b": "测试 B（Claude Code）"}
        for key_id, key_name in key_names.items():
            db.execute("INSERT INTO api_keys(id,status,created_at,last_used_at,name,key_hash,static_headers_json) VALUES(?, 'active', ?, ?, ?, ?, ?)", (key_id, day-86400, now, key_name, canary, canary))
            db.execute("INSERT INTO api_key_secrets VALUES(?,?)", (key_id, canary))
        db.execute("INSERT INTO tokens VALUES('internal-account',?,?)", (canary, canary))
        raw = [
            (1, "internal-key-a", "gpt-6-astra", 1, day+1, 90, 10, 20, 100, .1),
            (2, "internal-key-a", "gpt-6-astra", 0, day+2, 99999, 0, 99999, 199998, 99.0),
            (3, "internal-key-a", "gpt-6-astra", 1, day-86400, 180, 0, 20, 200, .2),
        ]
        db.executemany("INSERT INTO request_token_stats VALUES(?,?,?,?,?,?,?,?,?,?)", raw)
        db.execute("INSERT INTO request_token_stat_hourly_rollups VALUES('internal-key-a','gpt-6-astra',?,?,3,45,0,5,50,.05)", (day-2*86400,day-2*86400+3600))
        db.execute("INSERT INTO request_token_stat_rollups VALUES('internal-key-a','gpt-6-astra',4,360,0,40,400,.4)")
        db.execute("INSERT INTO models VALUES('model-astra','gpt-6-astra','GPT-6-Astra','Model description','builtin',1,'list',1,'passthrough',0,?)", (canary,))
        db.execute("INSERT INTO model_prices VALUES('model-astra','official',10000000,1000000,12500000,50000000,?)", (canary,))
        db.execute("INSERT INTO model_routes VALUES('model-astra',1,'account_pool',?,?)", (canary, canary))
    config = {"source_db": str(database), "id_secret": "ab"*32,
              "account_aliases": {"internal-account": "账号 01"},
              "key_aliases": {"internal-key-a": "同事 A", "internal-key-b": "同事 B"}}
    return {"config": config, "db": database, "now": now, "day": day, "canary": canary, "key_names": key_names}


@pytest.fixture
def web_files(sample, tmp_path):
    snapshot = tmp_path / "snapshot.json"
    write_snapshot(snapshot, collect(sample["config"], sample["now"]))
    password = "test-only-strong-password-9726"
    salt = "cd"*16
    auth = tmp_path / "auth.json"
    auth.write_text(json.dumps({"username": "team", "salt": salt, "password_hash": password_hash(password, salt),
                               "origins": ["http://127.0.0.1:48763", "https://stats.example.com"]}), encoding="utf-8")
    return {"snapshot": snapshot, "auth": auth, "password": password}
