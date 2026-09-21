import json
import sqlite3
import subprocess
import sys

import pytest

from dashboard.collector import collect, source_connection, write_snapshot, validate_output_path, public_id


def test_official_windows_and_whitelist(sample):
    result = collect(sample["config"], sample["now"])
    account = result["accounts"][0]
    assert account["plan"] == "pro"
    assert account["primary"]["minutes"] == 10080
    assert account["primary"]["remaining_percent"] == 75.0
    assert account["secondary"]["remaining_percent"] is None
    encoded = json.dumps(result)
    for forbidden in (sample["canary"], "internal-key", "internal-account", "key_value", "access_token", "credits_json"):
        assert forbidden not in encoded


def test_raw_hourly_legacy_and_failed_usage(sample):
    result = collect(sample["config"], sample["now"])
    totals = result["totals"]
    assert totals["today"]["total_tokens"] == 100
    assert totals["today"]["requests"] == 2
    assert totals["week"]["total_tokens"] == 350
    assert totals["week"]["requests"] == 6
    assert totals["recorded"]["total_tokens"] == 750
    assert totals["recorded"]["requests"] == 10
    assert totals["recorded"]["estimated_usd"] == pytest.approx(.75)
    key_id = public_id("key", "internal-key-b", sample["config"]["id_secret"])
    assert next(k for k in result["keys"] if k["id"] == key_id)["usage"]["recorded"]["total_tokens"] == 0


def test_current_key_names_are_exported_without_credentials_or_aliases(sample):
    result = collect(sample["config"], sample["now"])
    names = {public_id("key", key_id, sample["config"]["id_secret"]): name
             for key_id, name in sample["key_names"].items()}
    assert {key["id"]: key["label"] for key in result["keys"]} == names
    encoded = json.dumps(result, ensure_ascii=False)
    for forbidden in (sample["canary"], "key_hash", "key_value", "static_headers_json", "同事 A", "同事 B"):
        assert forbidden not in encoded


@pytest.mark.parametrize("name", ["张三", "示例团队 团队 / 测试 A（外网）", "研发组：李四 #02 & QA", "团队·CLI-test_v2.1"])
def test_key_names_preserve_chinese_spaces_and_punctuation(sample, name):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET name=?", (name,))
    assert all(key["label"] == name for key in collect(sample["config"], sample["now"])["keys"])


@pytest.mark.parametrize("name", [None, "", "   ", b"binary-name", "名字" * 65,
                                  "name\x00hidden", "name\nsecret", "<script>alert(1)</script>",
                                  "<img src=x onerror=alert(1)>", "https://example.com/?token=private",
                                  "sk-" + "a" * 30, "a1" * 32, "Bearer short-token", "api_key=short",
                                  "access_token:short", "refresh-token=short", "password=short",
                                  "eyJhbGciOiJIUzI1NiJ9.payload.signature", "person@example.com"])
def test_invalid_or_credential_like_key_names_keep_stable_codes(sample, name):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET name=?", (name,))
    keys = collect(sample["config"], sample["now"])["keys"]
    assert all(key["label"] == "Key " + key["id"][-6:] for key in keys)


def test_secret_canary_in_key_name_is_never_exported(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET name=?", (sample["canary"],))
    result = collect(sample["config"], sample["now"])
    assert sample["canary"] not in json.dumps(result, ensure_ascii=False)
    assert all(key["label"] == "Key " + key["id"][-6:] for key in result["keys"])


def test_key_name_refresh_preserves_identity_and_usage_without_mutating_source(sample):
    before = collect(sample["config"], sample["now"])
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET name='修改后的名称' WHERE id='internal-key-a'")
    source_before = sample["db"].read_bytes()
    after = collect(sample["config"], sample["now"])
    assert sample["db"].read_bytes() == source_before
    expected_id = public_id("key", "internal-key-a", sample["config"]["id_secret"])
    assert next(key for key in after["keys"] if key["id"] == expected_id)["label"] == "修改后的名称"
    for result in (before, after):
        for key in result["keys"]:
            key.pop("label")
    assert after == before


def test_deleted_key_is_not_listed_but_team_statistics_are_preserved(sample):
    before = collect(sample["config"], sample["now"])
    with sqlite3.connect(sample["db"]) as db:
        db.execute("DELETE FROM api_keys WHERE id='internal-key-a'")
    after = collect(sample["config"], sample["now"])
    assert after["totals"] == before["totals"]
    assert after["models_week"] == before["models_week"]
    key_id = public_id("key", "internal-key-a", sample["config"]["id_secret"])
    assert all(key["id"] != key_id and not key["is_historical"] for key in after["keys"])
    assert len(after["keys"]) == len(before["keys"]) - 1


@pytest.mark.parametrize("key_id", [None, "orphan-key"])
def test_unattributed_raw_and_archived_usage_never_adds_key_rows(sample, key_id):
    before = collect(sample["config"], sample["now"])
    with sqlite3.connect(sample["db"]) as db:
        for table in ("request_token_stats", "request_token_stat_hourly_rollups", "request_token_stat_rollups"):
            db.execute(f"UPDATE {table} SET key_id=?", (key_id,))
    after = collect(sample["config"], sample["now"])
    assert after["totals"] == before["totals"]
    assert after["models_week"] == before["models_week"]
    assert [key["id"] for key in after["keys"]] == [key["id"] for key in before["keys"]]
    assert all(not key["is_historical"] and key["usage"]["recorded"]["total_tokens"] == 0 for key in after["keys"])


def test_existing_disabled_key_keeps_its_usage_and_list_entry(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET status='disabled' WHERE id='internal-key-a'")
    result = collect(sample["config"], sample["now"])
    key_id = public_id("key", "internal-key-a", sample["config"]["id_secret"])
    key = next(k for k in result["keys"] if k["id"] == key_id)
    assert key["status"] == "disabled"
    assert key["is_historical"] is False
    assert key["usage"]["recorded"]["total_tokens"] == 750


def test_no_current_keys_still_retains_team_statistics(sample):
    before = collect(sample["config"], sample["now"])
    with sqlite3.connect(sample["db"]) as db:
        db.execute("DELETE FROM api_keys")
    after = collect(sample["config"], sample["now"])
    assert after["keys"] == []
    assert after["totals"] == before["totals"]
    assert after["models_week"] == before["models_week"]


def test_container_source_override_does_not_modify_private_config(sample, tmp_path):
    config_path = tmp_path / "collector.json"
    config_path.write_text(json.dumps(dict(sample["config"], source_db="not-a-container-path")), encoding="utf-8")
    original = config_path.read_bytes()
    snapshot = tmp_path / "snapshot.json"
    result = subprocess.run([sys.executable, "-m", "dashboard.collector", "--config", str(config_path),
                             "--source-db", str(sample["db"]), "--output", str(snapshot), "--once"],
                            capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert json.loads(snapshot.read_text(encoding="utf-8"))["totals"]["recorded"]["total_tokens"] == 750
    assert config_path.read_bytes() == original
    assert sample["canary"] not in result.stdout + result.stderr


def test_latest_snapshot_and_no_invented_reset(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("INSERT INTO usage_snapshots VALUES(2,'internal-account',80,10080,?,NULL,NULL,NULL,?,NULL)", (sample["now"]-10, sample["now"]))
    result = collect(sample["config"], sample["now"])
    assert result["accounts"][0]["primary"]["remaining_percent"] == 20.0


@pytest.mark.parametrize("sql", ["DELETE FROM accounts", "CREATE TABLE injected(id INT)",
                                  "ATTACH DATABASE ':memory:' AS other", "SELECT key_value FROM api_key_secrets",
                                  "SELECT access_token FROM tokens", "SELECT proxy_url FROM account_proxy_settings",
                                  "PRAGMA user_version=2", "SELECT load_extension('x')"])
def test_readonly_column_authorizer(sample, sql):
    with source_connection(sample["db"]) as db:
        with pytest.raises(sqlite3.DatabaseError):
            db.execute(sql).fetchall()


def test_bad_source_does_not_create_db_or_replace_good_snapshot(sample, tmp_path):
    output = tmp_path / "snapshot.json"
    write_snapshot(output, collect(sample["config"], sample["now"]))
    before = output.read_bytes()
    config = dict(sample["config"], source_db=str(tmp_path / "missing.db"))
    with pytest.raises(FileNotFoundError):
        write_snapshot(output, collect(config))
    assert not (tmp_path / "missing.db").exists()
    assert output.read_bytes() == before


def test_alias_and_model_canary_are_not_exported(sample):
    sample["config"]["account_aliases"]["internal-account"] = sample["canary"]
    sample["config"]["key_aliases"]["internal-key-a"] = "<script>alert(1)</script>"
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE request_token_stats SET model=?", (sample["canary"],))
    text = json.dumps(collect(sample["config"], sample["now"]))
    assert sample["canary"] not in text and "<script>" not in text


def test_total_fallback_does_not_double_count_cache(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE request_token_stats SET total_tokens=NULL WHERE request_log_id=1")
    assert collect(sample["config"], sample["now"])["totals"]["today"]["total_tokens"] == 100


def test_plausible_but_unapproved_model_is_not_exported(sample):
    fake_model = "gpt-INTERNALCONTACTALICE_CREDENTIAL_FRAGMENT"
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE request_token_stats SET model=?", (fake_model,))
    snapshot = collect(sample["config"], sample["now"])
    assert fake_model not in json.dumps(snapshot)
    assert any(item["model"] == "other" for item in snapshot["models_week"])


def test_live_wal_is_read_and_source_unchanged(sample):
    with sqlite3.connect(sample["db"]) as writer:
        writer.execute("PRAGMA journal_mode=WAL")
        writer.execute("UPDATE usage_snapshots SET used_percent=40 WHERE id=1")
        writer.commit()
        before = sample["db"].read_bytes()
        assert collect(sample["config"], sample["now"])["accounts"][0]["primary"]["remaining_percent"] == 60.0
        assert sample["db"].read_bytes() == before


def test_account_details_are_allowlisted_without_proxy_credentials(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE accounts SET label='person@example.com',sort=5")
        db.execute("UPDATE account_subscriptions SET expires_at=?,renews_at=?", (sample["now"]+86400, sample["now"]+86401))
        db.execute("INSERT INTO account_quota_capacity_overrides VALUES('internal-account',NULL,1000000)")
        db.execute("INSERT INTO account_proxy_settings VALUES('internal-account',1,?)", (sample["canary"],))
        db.execute("UPDATE usage_snapshots SET credits_json=?", (json.dumps({
            "_codexmanager_extra_rate_limits": [{"limit_id": "codex_other", "primary_window": {
                "used_percent": 0, "limit_window_seconds": 18000, "reset_at": sample["now"]+300},
                "secondary_window": {"used_percent": 10, "limit_window_seconds": 604800, "reset_at": sample["now"]+3600},
                "sensitive": sample["canary"]}],
            "rate_limit_reset_credits": {"available_count": 3, "credits": [{"id": sample["canary"], "status": "available"}]},
            "token": sample["canary"]}),))
    result = collect(sample["config"], sample["now"])
    account = result["accounts"][0]
    assert account["email"] == "person@example.com"
    assert account["subscription_expires_at"] == sample["now"]+86400
    assert account["sort_order"] == 5 and account["proxy_state"] == "configured"
    assert account["capacity_override"] is True and account["capacity_primary_tokens"] is None
    assert account["capacity_secondary_tokens"] == 1000000
    assert account["spark_primary"]["remaining_percent"] == 100
    assert account["spark_secondary"]["remaining_percent"] == 90
    assert account["primary"]["remaining_percent"] == 75
    assert account["reset_available_count"] == 3
    assert sample["canary"] not in json.dumps(result)


def test_new_optional_fields_are_unknown_without_data(sample):
    account = collect(sample["config"], sample["now"])["accounts"][0]
    assert account["email"] is None and account["subscription_expires_at"] is None
    assert account["proxy_state"] == "inherited" and account["sort_order"] == 0
    assert account["spark_primary"]["remaining_percent"] is None
    assert account["reset_available_count"] is None and account["capacity_override"] is False


def test_proxy_disabled_and_empty_override(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("INSERT INTO account_proxy_settings VALUES('internal-account',0,?)", (sample["canary"],))
        db.execute("INSERT INTO account_quota_capacity_overrides VALUES('internal-account',0,-10)")
    account = collect(sample["config"], sample["now"])["accounts"][0]
    assert account["proxy_state"] == "disabled" and account["capacity_override"] is False


@pytest.mark.parametrize("suffix", ["", "-wal", "-shm"])
def test_output_cannot_overwrite_source(sample, suffix):
    with pytest.raises(ValueError):
        validate_output_path(str(sample["db"])+suffix, sample["db"])
