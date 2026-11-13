"""Extract approved account fields; never export the source credits JSON or IDs."""

import json
import math
import re
from datetime import datetime


def first(source, *keys):
    return next((source[key] for key in keys if source.get(key) is not None), None)


def numeric(value):
    if isinstance(value, bool) or not isinstance(value, (int, float, str)):
        return None
    try:
        number = float(value)
        return number if math.isfinite(number) else None
    except (ValueError, OverflowError):
        return None


def positive_int(value):
    number = numeric(value)
    return int(number) if number is not None and 1 <= number <= 2**53-1 else None


def timestamp(value):
    number = numeric(value)
    if number is None and isinstance(value, str):
        try:
            parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
            number = parsed.timestamp() if parsed.tzinfo is not None else None
        except (ValueError, OverflowError, OSError):
            return None
    if number is not None and number > 1_000_000_000_000:
        number /= 1000
    return int(number) if number is not None and 0 < number < 253402300800 else None


def email_from_label(value):
    # Labels are editable. Accept a whole email only, never arbitrary notes/HTML.
    if not isinstance(value, str) or len(value) > 254:
        return None
    value = value.strip()
    if not re.fullmatch(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,63}", value):
        return None
    local, domain = value.rsplit("@", 1)
    if len(local) > 64 or local.startswith(".") or local.endswith(".") or ".." in value:
        return None
    if any(not part or len(part) > 63 or part.startswith("-") or part.endswith("-") for part in domain.split(".")):
        return None
    return value


def extra_window(raw):
    raw = raw if isinstance(raw, dict) else {}
    used = numeric(first(raw, "used_percent", "usedPercent"))
    remaining = numeric(first(raw, "remaining_percent", "remainingPercent"))
    if remaining is None and used is not None:
        remaining = 100 - used
    seconds = positive_int(first(raw, "limit_window_seconds", "limitWindowSeconds"))
    return {"minutes": math.ceil(seconds / 60) if seconds else None,
            "remaining_percent": float(max(0, min(100, remaining))) if remaining is not None else None,
            "resets_at": timestamp(first(raw, "reset_at", "resetAt"))}


def credits_details(raw, captured_at):
    result = {"spark_primary": extra_window(None), "spark_secondary": extra_window(None),
              "reset_available_count": None, "reset_next_expires_at": None}
    if not isinstance(raw, str) or len(raw) > 256 * 1024:
        return result
    try:
        payload = json.loads(raw)
    except (ValueError, RecursionError):
        return result
    if not isinstance(payload, dict):
        return result
    entries = []
    for key in ("_codexmanager_extra_rate_limits", "additional_rate_limits", "additionalRateLimits"):
        value = payload.get(key)
        if isinstance(value, list):
            entries.extend(("", item) for item in value[:100])
        elif isinstance(value, dict):
            entries.extend(list(value.items())[:100])
    for key in ("spark_rate_limit", "sparkRateLimit", "codex_other"):
        if key in payload:
            entries.append((key, payload[key]))
    for source_key, item in entries:
        if not isinstance(item, dict):
            continue
        container = first(item, "rate_limit", "rateLimit")
        container = container if isinstance(container, dict) else item
        identifiers = [source_key]
        for source in (item, container):
            identifiers.extend(first(source, *pair) for pair in (
                ("source_key", "sourceKey"), ("limit_id", "limitId"),
                ("limit_name", "limitName"), ("metered_feature", "meteredFeature")))
        if not any(isinstance(value, str) and ("spark" in value.lower() or value.lower() == "codex_other") for value in identifiers):
            continue
        # Only numeric window fields cross the boundary, not even a source label.
        result["spark_primary"] = extra_window(first(container, "primary_window", "primaryWindow"))
        result["spark_secondary"] = extra_window(first(container, "secondary_window", "secondaryWindow"))
        break
    resets = payload.get("rate_limit_reset_credits")
    if not isinstance(resets, dict):
        return result
    data = resets.get("data") if isinstance(resets.get("data"), dict) else {}
    count = first(resets, "available_count", "availableCount")
    if count is None:
        count = first(data, "available_count", "availableCount")
    parsed_count = numeric(count)
    if parsed_count is not None and abs(parsed_count) <= 2**53-1:
        result["reset_available_count"] = max(0, int(parsed_count))
    credits = resets.get("credits", data.get("credits"))
    if isinstance(credits, list) and len(credits) <= 1000:
        available = []
        for credit in credits:
            if not isinstance(credit, dict):
                continue
            state = first(credit, "status", "state")
            raw_expiry = first(credit, "expires_at", "expire_at", "expiresAt")
            expiry = timestamp(raw_expiry)
            if raw_expiry is not None and expiry is None:
                continue
            # Availability is historical as of the usage snapshot, not a live
            # entitlement. The viewer labels this as cached, never consumes it.
            if isinstance(state, str) and state.strip().lower() == "available" and (expiry is None or (captured_at and expiry > captured_at)):
                available.append(expiry)
        if result["reset_available_count"] is None and captured_at:
            result["reset_available_count"] = len(available)
        result["reset_next_expires_at"] = min((value for value in available if value), default=None)
    if result["reset_next_expires_at"] is None:
        result["reset_next_expires_at"] = timestamp(first(resets, "next_expires_at", "nextExpiresAt"))
    return result
