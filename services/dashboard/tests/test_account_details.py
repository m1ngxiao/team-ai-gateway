import json

import pytest

from dashboard.account_details import credits_details, email_from_label, timestamp


@pytest.mark.parametrize("raw", [None, "", "not-json", "[]", "null", "["*1200, "x"*300000],
                         ids=["missing", "empty", "invalid", "array", "null", "deep", "oversize"])
def test_bad_json_does_not_expose_or_invent_quota(raw):
    result = credits_details(raw, 100)
    assert result["spark_primary"]["remaining_percent"] is None
    assert result["reset_available_count"] is None


@pytest.mark.parametrize("value,expected", [(0,0), (3,3), ("2",2), (2.8,2), (-1,0), (True,None), ("NaN",None), (10**100,None)])
def test_cached_reset_count_normalization(value, expected):
    raw = json.dumps({"rate_limit_reset_credits": {"available_count": value}})
    assert credits_details(raw, 100)["reset_available_count"] == expected


def test_reset_credit_details_are_historical_not_live_entitlements():
    credits = [{"id": "never-export-id", "status": "available", "expires_at": 200},
               {"status": "available", "expires_at": 90}, {"status": "redeemed"},
               {"status": "available", "expires_at": "bad-date"}]
    raw = json.dumps({"rate_limit_reset_credits": {"credits": credits}})
    result = credits_details(raw, 100)
    assert result["reset_available_count"] == 1 and result["reset_next_expires_at"] == 200
    assert "never-export-id" not in json.dumps(result)
    raw = json.dumps({"rate_limit_reset_credits": {"available_count": None, "availableCount": 2}})
    assert credits_details(raw, 100)["reset_available_count"] == 2


def test_camel_case_spark_windows_no_reset_invention_and_unknown_pool_not_echoed():
    raw = json.dumps({"additionalRateLimits": {"codex_other": {"rateLimit": {
        "primaryWindow": {"remainingPercent": 8, "limitWindowSeconds": 18000, "resetAt": 50},
        "secondaryWindow": {"usedPercent": "NaN"}}}, "SECRET": {"primary_window": {"used_percent": 0}}}})
    result = credits_details(raw, 100)
    assert result["spark_primary"] == {"minutes": 300, "remaining_percent": 8.0, "resets_at": 50}
    assert result["spark_secondary"]["remaining_percent"] is None
    assert "SECRET" not in json.dumps(result)


@pytest.mark.parametrize("label", ["token: secret@example.com", "<b>a@example.com</b>", "a\n@example.com", "a@-example.com", "a..b@example.com", "a@bad..com"])
def test_arbitrary_database_labels_are_not_public_email(label):
    assert email_from_label(label) is None


def test_timestamp_supports_db_seconds_milliseconds_and_explicit_timezone():
    assert timestamp(1700000000000) == timestamp(1700000000) == 1700000000
    assert timestamp("2023-11-14T22:13:20Z") == 1700000000
    assert timestamp("2023-11-14T22:13:20") is None
