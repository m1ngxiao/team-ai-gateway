import json
import sqlite3

import pytest

from dashboard.catalog import DESCRIPTION_ZH, price_value, public_text
from dashboard.collector import collect, source_connection, public_id
from dashboard.models import Snapshot


def test_catalog_uses_local_fields_and_correct_price_units(sample):
    data = collect(sample["config"], sample["now"])
    model = data["model_catalog"][0]
    assert model["model"] == "gpt-6-astra" and model["origin"] == "builtin"
    assert model["enabled"] is True and model["instructions_mode"] == "passthrough"
    assert model["price"] == {"status": "official", "input_usd_per_million": 10.0,
        "cached_input_usd_per_million": 1.0, "cache_write_usd_per_million": 12.5, "output_usd_per_million": 50.0}
    assert model["route_count"] == model["account_pool_routes"] == 1
    assert model["aggregate_api_routes"] == 0
    assert sample["canary"] not in json.dumps(data)


@pytest.mark.parametrize("cache,expected", [(None,10.0), (0,0.0), (1000000,1.0)])
def test_cache_write_null_fallback_not_truthiness(sample, cache, expected):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE model_prices SET cache_write_microusd_per_1m=?", (cache,))
    assert collect(sample["config"], sample["now"])["model_catalog"][0]["price"]["cache_write_usd_per_million"] == expected


@pytest.mark.parametrize("damage", ["missing-status", "no-price-row"])
def test_missing_price_is_not_free_and_model_is_kept(sample, damage):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("DELETE FROM model_prices" if damage == "no-price-row" else "UPDATE model_prices SET price_status='missing'")
    item = collect(sample["config"], sample["now"])["model_catalog"][0]
    assert item["price"]["status"] == "missing"
    assert item["price"]["input_usd_per_million"] is None


def test_hidden_unapproved_and_route_counts_are_safe(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("INSERT INTO models VALUES('hidden','codex-auto-review','hidden','', 'builtin',1,'hide',1,'override',1,?)", (sample["canary"],))
        db.execute("INSERT INTO models VALUES('unknown',?,?,?,'custom',1,'list',1,'override',2,?)", ("gpt-PRIVATE_TOKEN", sample["canary"], sample["canary"], sample["canary"]))
        db.execute("INSERT INTO model_routes VALUES('model-astra',0,'account_pool',?,?)", (sample["canary"],sample["canary"]))
        db.execute("INSERT INTO model_routes VALUES('model-astra',1,'aggregate_api',?,?)", (sample["canary"],sample["canary"]))
    data = collect(sample["config"], sample["now"])
    assert len(data["model_catalog"]) == 1 and data["catalog_unlisted_count"] == 1
    assert data["model_catalog"][0]["route_count"] == 2
    assert data["model_catalog"][0]["aggregate_api_routes"] == 1
    assert sample["canary"] not in json.dumps(data) and "gpt-PRIVATE_TOKEN" not in json.dumps(data)


def test_disabled_model_stays_disabled_and_catalog_does_not_invent_routes(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE models SET enabled=0,instructions_mode='override',supported_in_api=0")
        db.execute("DELETE FROM model_routes")
    item = collect(sample["config"], sample["now"])["model_catalog"][0]
    assert item["enabled"] is False and item["supported_in_api"] is False
    assert item["instructions_mode"] == "override" and item["route_count"] == 0


def test_key_name_identifier_protocol_binding_and_token_limit(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET name='外网 CLI 测试',model_slug='gpt-5.6-sol',rotation_strategy='hybrid_rotation' WHERE id='internal-key-a'")
        db.execute("INSERT INTO api_key_profiles VALUES('internal-key-a','anthropic','gpt-6-astra',?)", (sample["canary"],))
        db.execute("INSERT INTO api_key_quota_limits VALUES('internal-key-a',500)")
        db.execute("INSERT INTO api_keys(id,name,status,created_at) VALUES('gk_123456789abc','本机测试','active',?)", (sample["now"],))
    data = collect(sample["config"], sample["now"])
    key_id = public_id("key", "internal-key-a", sample["config"]["id_secret"])
    key = next(item for item in data["keys"] if item["id"] == key_id)
    assert key["protocol"] == "anthropic_native" and key["rotation"] == "hybrid_rotation"
    assert key["model_binding"] == "fixed" and key["bound_model"] == "gpt-6-astra"
    assert key["quota_limit_tokens"] == 500 and key["usage"]["recorded"]["total_tokens"] == 750
    assert key["quota_config_known"] is True
    assert key["label"] == "外网 CLI 测试"
    assert any(item["display_id"] == "gk_123456789abc" and item["label"] == "本机测试" for item in data["keys"])
    for private_name in (sample["canary"], "同事 A", "同事 B"):
        assert private_name not in json.dumps(data, ensure_ascii=False)


def test_unknown_binding_and_metadata_are_not_echoed(sample):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE api_keys SET model_slug=?,rotation_strategy=?", (sample["canary"],sample["canary"]))
    data = collect(sample["config"], sample["now"])
    assert data["keys"][0]["model_binding"] == "unlisted" and data["keys"][0]["bound_model"] is None
    assert data["keys"][0]["rotation"] == "unknown"
    assert sample["canary"] not in json.dumps(data)


def test_key_name_column_is_readable_without_opening_secret_columns(sample):
    with source_connection(sample["db"]) as db:
        names = {row["name"] for row in db.execute("SELECT name FROM api_keys")}
    assert names == set(sample["key_names"].values())


@pytest.mark.parametrize("sql", ["SELECT key_hash FROM api_keys", "SELECT static_headers_json FROM api_keys",
    "SELECT upstream_base_url FROM api_key_profiles", "SELECT price_source FROM model_prices",
    "SELECT source_id FROM model_routes", "SELECT upstream_model FROM model_routes", "SELECT instructions_text FROM models"])
def test_new_queries_do_not_open_secret_columns(sample, sql):
    with source_connection(sample["db"]) as db:
        with pytest.raises(sqlite3.DatabaseError):
            db.execute(sql).fetchall()


@pytest.mark.parametrize("value", [True, -1, 0.5, "1", float("inf"), 10**100])
def test_price_requires_finite_integer_microusd(value):
    assert price_value(value) is None


@pytest.mark.parametrize("value", ["hf_"+"a"*34, "AIza"+"a"*35, "api_key=short", "access_token:short", "<img src=x>"])
def test_obvious_secrets_in_names_are_rejected(value):
    assert public_text(value, "fallback") == "fallback"


def test_old_snapshot_remains_compatible(sample):
    data = collect(sample["config"], sample["now"])
    del data["model_catalog"]
    del data["catalog_unlisted_count"]
    for key in data["keys"]:
        for name in ("display_id", "protocol", "rotation", "model_binding", "bound_model", "quota_limit_tokens", "quota_config_known", "is_historical"):
            del key[name]
    parsed = Snapshot.model_validate(data)
    assert parsed.model_catalog == [] and parsed.keys[0].quota_config_known is False


@pytest.mark.parametrize("description", list(DESCRIPTION_ZH), ids=["astra", "sol", "terra", "luna", "5.5", "5.4", "mini", "5.2", "image"])
def test_builtin_descriptions_are_translated_without_changing_model_ids(sample, description):
    with sqlite3.connect(sample["db"]) as db:
        db.execute("UPDATE models SET description=?", (description,))
    model = collect(sample["config"], sample["now"])["model_catalog"][0]
    assert model["description"] == DESCRIPTION_ZH[description]
    assert model["model"] == "gpt-6-astra"
