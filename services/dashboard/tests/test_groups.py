import json
import sqlite3

import pytest

from dashboard.collector import FIELDS, collect, group_identity, source_connection
from dashboard.models import Snapshot


def snapshot(sample):
    return collect(sample["config"], sample["now"])


def assert_reconciles(data):
    for period, total in data["totals"].items():
        for field in FIELDS:
            value = sum(group["usage"][period][field] for group in data["key_groups"])
            assert value == pytest.approx(total[field]), (period, field)


def test_current_key_groups_include_zero_use_and_disabled_keys(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE accounts SET group_name=' 研发组 B（示例账号） '")
        db.execute("UPDATE api_keys SET account_group_filter=' 研发组 B（示例账号） ' WHERE id='internal-key-a'")
        db.execute("UPDATE api_keys SET account_group_filter='示例团队',status='disabled' WHERE id='internal-key-b'")
    before = sample["db"].read_bytes()
    data = snapshot(sample)
    assert sample["db"].read_bytes() == before
    assert data["accounts"][0]["group_name"] == "研发组 B（示例账号）"
    assert {key["group_name"] for key in data["keys"]} == {"研发组 B（示例账号）", "示例团队"}
    groups = {group["label"]: group for group in data["key_groups"]}
    assert groups["研发组 B（示例账号）"]["key_count"] == 1
    assert groups["研发组 B（示例账号）"]["usage"]["recorded"]["total_tokens"] == 750
    assert groups["示例团队"]["key_count"] == 1
    assert groups["示例团队"]["usage"]["recorded"]["requests"] == 0
    assert all(group["kind"] == "named" for group in groups.values())
    assert_reconciles(data)


def test_same_group_sums_multiple_keys_and_all_archive_types(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET account_group_filter='团队 A'")
        db.execute("INSERT INTO request_token_stats VALUES(4,'internal-key-b','gpt-6-astra',1,?,40,10,5,35,.04)",
                   (sample["day"] + 3,))
    data = snapshot(sample)
    assert len(data["key_groups"]) == 1
    group = data["key_groups"][0]
    assert group["key_count"] == 2
    assert group["usage"]["today"]["total_tokens"] == 135
    assert group["usage"]["today"]["requests"] == 3  # Includes the request with usage_included=0.
    assert group["usage"]["week"]["total_tokens"] == 385
    assert group["usage"]["recorded"]["total_tokens"] == 785
    assert_reconciles(data)


def test_reassignment_moves_history_without_changing_personal_or_team_totals(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET account_group_filter='旧组'")
    old = snapshot(sample)
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET account_group_filter='新组' WHERE id='internal-key-a'")
    new = snapshot(sample)
    groups = {group["label"]: group for group in new["key_groups"]}
    assert groups["旧组"]["usage"]["recorded"]["total_tokens"] == 0
    assert groups["新组"]["usage"]["recorded"] == old["totals"]["recorded"]
    assert new["totals"] == old["totals"]
    assert {key["id"]: key["usage"] for key in new["keys"]} == {key["id"]: key["usage"] for key in old["keys"]}
    assert_reconciles(new)


def test_group_usage_follows_key_membership_not_upstream_account_group(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE accounts SET group_name='上游组'")
        db.execute("UPDATE api_keys SET account_group_filter='成员组'")
    data = snapshot(sample)
    assert data["accounts"][0]["group_name"] == "上游组"
    assert [group["label"] for group in data["key_groups"]] == ["成员组"]
    assert_reconciles(data)


def test_unrestricted_deleted_and_unattributed_keys_are_distinct(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET account_group_filter='  '")
        db.execute("DELETE FROM api_keys WHERE id='internal-key-a'")
        db.execute("INSERT INTO request_token_stats VALUES(4,NULL,'gpt-6-astra',1,?,5,0,0,5,.01)", (sample["day"]+3,))
    data = snapshot(sample)
    groups = {group["kind"]: group for group in data["key_groups"]}
    assert data["keys"][0]["group_name"] is None
    assert groups["unrestricted"]["key_count"] == 1
    assert groups["unrestricted"]["usage"]["recorded"]["total_tokens"] == 0
    assert groups["unattributed"]["key_count"] == 0
    assert groups["unattributed"]["usage"]["recorded"]["total_tokens"] == 755
    assert_reconciles(data)


def test_empty_database_has_no_phantom_groups(sample):
    with sqlite3.connect(sample["db"]) as db:
        for table in ("api_keys", "request_token_stats", "request_token_stat_hourly_rollups", "request_token_stat_rollups"):
            db.execute("DELETE FROM " + table)
    data = snapshot(sample)
    assert data["key_groups"] == []
    assert_reconciles(data)


@pytest.mark.parametrize("value", ["<script>alert(1)</script>", "person@example.com", "sk-" + "x" * 40,
                                   "password=secret", "https://example.com/?token=private", "x" * 129,
                                   "bad\nname", b"binary"])
def test_unsafe_group_names_are_redacted_consistently_without_changing_identity(sample, value):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE accounts SET group_name=?", (value,))
        db.execute("UPDATE api_keys SET account_group_filter=? WHERE id='internal-key-a'", (value,))
        db.execute("UPDATE api_keys SET account_group_filter=? WHERE id='internal-key-b'", ("<different-unsafe-group>",))
    data = snapshot(sample)
    groups = data["key_groups"]
    assert len(groups) == 2
    assert len({group["id"] for group in groups}) == 2
    assert len({group["label"] for group in groups}) == 2
    assert all(group["label"].startswith("分组 ") for group in groups)
    assert data["accounts"][0]["group_name"] in {group["label"] for group in groups}
    encoded = json.dumps(data, ensure_ascii=False)
    assert "different-unsafe-group" not in encoded
    if isinstance(value, str):
        assert value not in encoded
    assert_reconciles(data)


def test_group_names_are_exact_and_reserved_labels_cannot_merge_buckets(sample):
    secret = sample["config"]["id_secret"]
    assert group_identity(" Team-A ", secret) == group_identity("Team-A", secret)
    assert group_identity("team-a", secret)[0] != group_identity("Team-A", secret)[0]
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET account_group_filter='全部分组（未限制）' WHERE id='internal-key-a'")
    data = snapshot(sample)
    assert len({group["id"] for group in data["key_groups"]}) == 2
    assert {group["kind"] for group in data["key_groups"]} == {"named", "unrestricted"}
    assert_reconciles(data)


def test_group_fields_default_for_previous_snapshots(sample):
    data = snapshot(sample)
    del data["key_groups"]
    for item in data["accounts"] + data["keys"]:
        del item["group_name"]
    parsed = Snapshot.model_validate(data)
    assert parsed.key_groups == []
    assert all(item.group_name is None for item in parsed.accounts + parsed.keys)


def test_only_explicit_group_columns_are_allowed(sample):
    with source_connection(sample["db"]) as db:
        db.execute("SELECT group_name FROM accounts").fetchall()
        db.execute("SELECT account_group_filter FROM api_keys").fetchall()
        for sql in ("SELECT key_hash FROM api_keys", "SELECT access_token FROM tokens", "UPDATE accounts SET group_name='changed'"):
            with pytest.raises(sqlite3.DatabaseError):
                db.execute(sql)
