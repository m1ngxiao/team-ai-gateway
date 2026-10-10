"""Dynamic catalog regressions using only synthetic models and in-memory data."""

import json
import sqlite3
import unittest
from contextlib import contextmanager
from unittest.mock import patch

from dashboard.catalog import collect_catalog, key_metadata, public_model_slug
from dashboard.collector import collect, read_authorizer


EXISTING_MODELS = {
    "claude-sonnet-5", "gpt-6-astra", "gpt-6.1-sol", "gpt-6-sol", "gpt-6-luna",
    "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.5", "gpt-5.4",
    "gpt-5.4-mini", "gpt-5.3-codex", "gpt-5.3-codex-spark", "gpt-5.2",
    "gpt-5.2-codex", "gpt-5.1-codex-max", "gpt-5.1-codex-mini", "gpt-reserve",
    "codex-auto-review", "gpt-image-2",
}


def database():
    db = sqlite3.connect(":memory:")
    db.row_factory = sqlite3.Row
    db.executescript("""
      CREATE TABLE models(id TEXT PRIMARY KEY,slug TEXT,display_name TEXT,description TEXT,
        origin TEXT,enabled INT,visibility TEXT,supported_in_api INT,instructions_mode TEXT,
        sort_order INT,instructions_text TEXT);
      CREATE TABLE model_prices(model_id TEXT PRIMARY KEY,price_status TEXT,
        input_microusd_per_1m INT,cached_input_microusd_per_1m INT,
        cache_write_microusd_per_1m INT,output_microusd_per_1m INT,price_source TEXT);
      CREATE TABLE model_routes(model_id TEXT,enabled INT,source_kind TEXT,source_id TEXT,
        upstream_model TEXT);
      CREATE TABLE accounts(id TEXT PRIMARY KEY,status TEXT,created_at INT,label TEXT,
        sort INT,updated_at INT,group_name TEXT);
      CREATE TABLE account_subscriptions(account_id TEXT,account_plan_type TEXT,plan_type TEXT,
        expires_at INT,renews_at INT);
      CREATE TABLE account_quota_capacity_overrides(account_id TEXT,primary_window_tokens INT,
        secondary_window_tokens INT);
      CREATE TABLE account_proxy_settings(account_id TEXT,enabled INT,proxy_url TEXT);
      CREATE TABLE usage_snapshots(id INT,account_id TEXT,used_percent REAL,window_minutes INT,
        resets_at INT,secondary_used_percent REAL,secondary_window_minutes INT,
        secondary_resets_at INT,captured_at INT,credits_json TEXT);
      CREATE TABLE api_keys(id TEXT PRIMARY KEY,status TEXT,created_at INT,last_used_at INT,
        name TEXT,rotation_strategy TEXT,model_slug TEXT,account_group_filter TEXT,
        upstream_provider TEXT);
      CREATE TABLE api_key_profiles(key_id TEXT,protocol_type TEXT,default_model TEXT);
      CREATE TABLE api_key_quota_limits(key_id TEXT,quota_limit_tokens INT);
      CREATE TABLE request_token_stats(request_log_id INT,key_id TEXT,model TEXT,
        usage_included INT,created_at INT,input_tokens INT,cached_input_tokens INT,
        output_tokens INT,total_tokens INT,estimated_cost_usd REAL,actual_source_kind TEXT);
      CREATE TABLE request_token_stat_hourly_rollups(key_id TEXT,model TEXT,bucket_start INT,
        bucket_end INT,request_count INT,input_tokens INT,cached_input_tokens INT,
        output_tokens INT,total_tokens INT,estimated_cost_usd REAL,actual_source_kind TEXT);
      CREATE TABLE request_token_stat_rollups(key_id TEXT,model TEXT,source_rows INT,
        input_tokens INT,cached_input_tokens INT,output_tokens INT,total_tokens INT,
        estimated_cost_usd REAL);
    """)
    return db


def model(db, slug, *, enabled=1, supported=1, visibility="list", origin="builtin",
          display_name=None, description="", order=0):
    db.execute("INSERT INTO models VALUES(?,?,?,?,?,?,?,?,?,?,?)", (
        slug, slug, display_name or slug, description, origin, enabled, visibility,
        supported, "passthrough", order, "PRIVATE-INSTRUCTIONS-NOT-EXPORTED",
    ))


def full_snapshot(db):
    @contextmanager
    def connection(_path):
        db.set_authorizer(read_authorizer)
        yield db
    config = {"source_db": "/unused/synthetic.db", "id_secret": "ab" * 32}
    with patch("dashboard.collector.source_connection", connection):
        return collect(config, now=1800000000)


class DynamicCatalogTests(unittest.TestCase):
    def setUp(self):
        self.db = database()
        self.addCleanup(self.db.close)

    def test_all_existing_models_and_future_providers_are_kept(self):
        future = {"gpt-6.2-sol", "anthropic/claude-next", "google/gemini-next", "vendor:new-model"}
        for slug in EXISTING_MODELS | future:
            model(self.db, slug)
        items, unlisted = collect_catalog(self.db)
        self.assertEqual({item["model"] for item in items}, EXISTING_MODELS | future)
        self.assertEqual(unlisted, 0)
        self.assertTrue(all(item["enabled"] and item["supported_in_api"] for item in items))
        self.assertTrue(all(item["price"] == {"status": "missing"} for item in items))

    def test_disabled_api_disabled_and_hidden_visibility_are_preserved(self):
        model(self.db, "vendor-disabled", enabled=0, origin="custom")
        model(self.db, "vendor-not-api", supported=0, origin="custom")
        model(self.db, "vendor-hidden", visibility="hide")
        items, unlisted = collect_catalog(self.db)
        indexed = {item["model"]: item for item in items}
        self.assertEqual(set(indexed), {"vendor-disabled", "vendor-not-api"})
        self.assertFalse(indexed["vendor-disabled"]["enabled"])
        self.assertFalse(indexed["vendor-not-api"]["supported_in_api"])
        self.assertEqual(indexed["vendor-disabled"]["origin"], "custom")
        self.assertEqual(unlisted, 0)

    def test_unsafe_slugs_are_counted_and_not_echoed(self):
        invalid = ["sk-" + "a" * 45, "https://example.test/private", "Bearer synthetic",
                   "api_key=synthetic", "model\nsecret", "<script>", "x" * 129]
        for slug in invalid:
            model(self.db, slug)
        items, unlisted = collect_catalog(self.db)
        self.assertEqual(items, [])
        self.assertEqual(unlisted, len(invalid))
        for slug in invalid:
            self.assertNotIn(slug, json.dumps(items))

    def test_names_and_descriptions_remain_sanitized(self):
        model(self.db, "gpt-future", display_name="api_key=synthetic",
              description="https://example.test/private")
        item = collect_catalog(self.db)[0][0]
        self.assertEqual(item["name"], "gpt-future")
        self.assertEqual(item["description"], "")
        self.assertNotIn("PRIVATE-INSTRUCTIONS-NOT-EXPORTED", json.dumps(item))

    def test_missing_price_row_is_not_free(self):
        model(self.db, "gpt-future")
        self.assertEqual(collect_catalog(self.db)[0][0]["price"], {"status": "missing"})

    def test_missing_cache_write_price_is_not_invented(self):
        model(self.db, "gpt-future")
        self.db.execute("INSERT INTO model_prices VALUES(?,?,?,?,?,?,?)", (
            "gpt-future", "official", 1250000, 0, None, 5000000, "PRIVATE-PRICE-SOURCE",
        ))
        price = collect_catalog(self.db)[0][0]["price"]
        self.assertEqual(price, {"status": "official", "input_usd_per_million": 1.25,
            "cached_input_usd_per_million": 0.0, "cache_write_usd_per_million": None,
            "output_usd_per_million": 5.0})

    def test_unknown_status_and_invalid_numeric_price_are_not_fabricated(self):
        model(self.db, "gpt-future")
        self.db.execute("INSERT INTO model_prices VALUES(?,?,?,?,?,?,?)", (
            "gpt-future", "official", -1, None, 0, 1.5, "PRIVATE-PRICE-SOURCE",
        ))
        price = collect_catalog(self.db)[0][0]["price"]
        self.assertIsNone(price["input_usd_per_million"])
        self.assertIsNone(price["cached_input_usd_per_million"])
        self.assertEqual(price["cache_write_usd_per_million"], 0.0)
        self.assertIsNone(price["output_usd_per_million"])
        self.db.execute("UPDATE model_prices SET price_status='unverified'")
        self.assertEqual(collect_catalog(self.db)[0][0]["price"], {"status": "missing"})

    def test_routes_count_only_enabled_routes_and_export_no_route_credentials(self):
        model(self.db, "claude-next")
        self.db.executemany("INSERT INTO model_routes VALUES(?,?,?,?,?)", [
            ("claude-next", 1, "account_pool", "PRIVATE-POOL", "PRIVATE-UPSTREAM"),
            ("claude-next", 1, "aggregate_api", "PRIVATE-POOL", "PRIVATE-UPSTREAM"),
            ("claude-next", 0, "aggregate_api", "PRIVATE-POOL", "PRIVATE-UPSTREAM"),
        ])
        item = collect_catalog(self.db)[0][0]
        self.assertEqual(item["route_count"], 2)
        self.assertEqual(item["account_pool_routes"], 1)
        self.assertEqual(item["aggregate_api_routes"], 1)
        self.assertNotIn("PRIVATE-", json.dumps(item))

    def test_sql_authorizer_still_forbids_secret_columns(self):
        model(self.db, "gpt-future")
        self.db.set_authorizer(read_authorizer)
        self.assertEqual(collect_catalog(self.db)[0][0]["model"], "gpt-future")
        for sql in ["SELECT instructions_text FROM models", "SELECT source_id FROM model_routes",
                    "SELECT upstream_model FROM model_routes", "SELECT price_source FROM model_prices"]:
            with self.subTest(sql=sql), self.assertRaises(sqlite3.DatabaseError):
                self.db.execute(sql).fetchall()

    def test_future_model_key_binding_and_usage_follow_catalog(self):
        model(self.db, "gpt-6.2-sol")
        self.db.execute("INSERT INTO api_keys VALUES(?,?,?,?,?,?,?,?,?)", (
            "gk_123456789abc", "active", 1799990000, None, "Synthetic key", "account_rotation",
            "gpt-6.2-sol", None, "openai",
        ))
        self.db.executemany("INSERT INTO request_token_stats VALUES(?,?,?,?,?,?,?,?,?,?,?)", [
            (1, "gk_123456789abc", "gpt-6.2-sol", 1, 1799999999, 10, 0, 5, 15, 0.0, "openai_account"),
            (2, "gk_123456789abc", "unregistered-request-model", 1, 1799999999, 10, 0, 5, 15, 0.0, "openai_account"),
            (3, "gk_123456789abc", "sk-" + "a" * 45, 1, 1799999999, 10, 0, 5, 15, 0.0, None),
        ])
        data = full_snapshot(self.db)
        key = data["keys"][0]
        self.assertEqual((key["model_binding"], key["bound_model"]), ("fixed", "gpt-6.2-sol"))
        self.assertEqual(key["upstream_provider"], "openai")
        by_model = {item["model"]: item["usage"]["total_tokens"] for item in data["models_week"]}
        self.assertEqual(by_model, {"gpt-6.2-sol": 15, "other": 30})
        self.assertEqual(data["totals"]["recorded"]["total_tokens"], 45)
        self.assertEqual(data["pool_usage"]["openai"]["recorded"]["total_tokens"], 30)
        self.assertEqual(data["pool_usage"]["unattributed"]["recorded"]["total_tokens"], 15)
        self.assertNotIn("unregistered-request-model", json.dumps(data))
        self.assertNotIn("sk-" + "a" * 45, json.dumps(data))

    def test_unregistered_fixed_binding_is_not_echoed(self):
        row = {"id": "gk_123456789abc", "protocol_type": "openai_compat",
               "rotation_strategy": "account_rotation", "model_slug": "unregistered-private",
               "quota_limit_tokens": None, "upstream_provider": "openai"}
        data = key_metadata(row, {"gpt-future"})
        self.assertEqual(data["model_binding"], "unlisted")
        self.assertIsNone(data["bound_model"])

    def test_hidden_catalog_ids_are_not_exposed_in_bindings_or_usage(self):
        model(self.db, "gpt-hidden", visibility="hide")
        model(self.db, "vendor-disabled", enabled=0)
        self.db.execute("INSERT INTO api_keys VALUES(?,?,?,?,?,?,?,?,?)", (
            "gk_123456789abc", "active", 1799990000, None, "Synthetic key", "account_rotation",
            "gpt-hidden", None, "openai",
        ))
        self.db.executemany("INSERT INTO request_token_stats VALUES(?,?,?,?,?,?,?,?,?,?,?)", [
            (1, "gk_123456789abc", "gpt-hidden", 1, 1799999999, 10, 0, 5, 15, 0.0, "openai_account"),
            (2, "gk_123456789abc", "vendor-disabled", 1, 1799999999, 10, 0, 5, 15, 0.0, "openai_account"),
        ])
        data = full_snapshot(self.db)
        self.assertNotIn("gpt-hidden", json.dumps(data))
        self.assertEqual(data["keys"][0]["model_binding"], "unlisted")
        self.assertIsNone(data["keys"][0]["bound_model"])
        self.assertEqual(data["model_catalog"][0]["model"], "vendor-disabled")
        self.assertFalse(data["model_catalog"][0]["enabled"])
        self.assertEqual({item["model"]: item["usage"]["total_tokens"] for item in data["models_week"]},
                         {"other": 15, "vendor-disabled": 15})
        self.assertEqual(data["totals"]["recorded"]["total_tokens"], 30)

    def test_model_slug_validation_preserves_namespaces(self):
        for slug in ["gpt-6.2-sol", "anthropic/claude-next", "vendor:model-v2", "gpt-5.4-mini"]:
            with self.subTest(slug=slug):
                self.assertEqual(public_model_slug(slug), slug)
        for slug in [None, 123, "model id", " model", "model@host", "api_key=synthetic"]:
            with self.subTest(slug=slug):
                self.assertIsNone(public_model_slug(slug))


if __name__ == "__main__":
    unittest.main(verbosity=2)
