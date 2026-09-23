"""Schema-pinned, credential-free catalogue and platform-key presentation fields."""

import re

from .account_details import positive_int

PUBLIC_MODELS = frozenset({"claude-sonnet-5", "gpt-6-astra", "gpt-6-sol", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna", "gpt-5.5",
                          "gpt-5.4", "gpt-5.4-mini", "gpt-5.3-codex", "gpt-5.3-codex-spark",
                          "gpt-5.2", "gpt-5.2-codex", "gpt-5.1-codex-max", "gpt-5.1-codex-mini",
                          "gpt-reserve", "codex-auto-review", "gpt-image-2"})

# Translate the known source copy, not arbitrary changed descriptions by slug.
DESCRIPTION_ZH = {
    "Our most capable model for complex, demanding work.": "能力最强的模型，适合复杂、高要求的任务。",
    "Built to power complex coding and agentic workflows.": "适合复杂编程和智能体工作流。",
    "Latest frontier agentic coding model.": "最新前沿模型，适合自主编程。",
    "Balanced agentic coding model for everyday work.": "能力均衡的编程模型，适合日常工作。",
    "Fast and affordable agentic coding model.": "快速、经济的自主编程模型。",
    "Frontier model for complex coding, research, and real-world work.": "适合复杂编程、研究和实际工作。",
    "Strong model for everyday coding.": "能力出色的日常编程模型。",
    "Small, fast, and cost-efficient model for simpler coding tasks.": "小巧、快速且经济，适合简单编程任务。",
    "Optimized for professional work and long-running agents.": "适合专业工作和需要长时间执行的任务。",
    "State-of-the-art image generation and editing model.": "先进的图像生成与编辑模型。",
}

CATALOG_COLUMNS = {
    "models": {"id", "slug", "display_name", "description", "origin", "enabled", "visibility",
               "supported_in_api", "instructions_mode", "sort_order"},
    "model_prices": {"model_id", "price_status", "input_microusd_per_1m", "cached_input_microusd_per_1m",
                     "cache_write_microusd_per_1m", "output_microusd_per_1m"},
    "model_routes": {"model_id", "enabled", "source_kind"},
}

CATALOG_SQL = """
SELECT m.slug,m.display_name,m.description,m.origin,m.enabled,m.supported_in_api,m.instructions_mode,
  p.price_status,p.input_microusd_per_1m,p.cached_input_microusd_per_1m,
  p.cache_write_microusd_per_1m,p.output_microusd_per_1m,
  COALESCE(r.route_count,0) AS route_count,COALESCE(r.account_pool_count,0) AS account_pool_count,
  COALESCE(r.aggregate_api_count,0) AS aggregate_api_count
FROM models m LEFT JOIN model_prices p ON p.model_id=m.id
LEFT JOIN (
  SELECT model_id,COUNT(model_id) AS route_count,
    SUM(CASE WHEN source_kind='account_pool' THEN 1 ELSE 0 END) AS account_pool_count,
    SUM(CASE WHEN source_kind='aggregate_api' THEN 1 ELSE 0 END) AS aggregate_api_count
  FROM model_routes WHERE enabled=1 GROUP BY model_id
) r ON r.model_id=m.id
WHERE m.visibility='list'
ORDER BY m.sort_order,m.slug
"""


def public_text(value, fallback="", maximum=128):
    """Names/descriptions are in scope, but obvious credential/URL blobs are not."""
    if not isinstance(value, str):
        return fallback
    value = value.strip()
    if not value or len(value) > maximum or any(ord(char) < 32 for char in value):
        return fallback
    if re.search(r"[<>@]|https?://|\b(?:sk-|hf_|gh[pousr]_|github_pat_|AIza|Bearer\s|eyJ)|(?:api[_ -]?key|access[_ -]?token|refresh[_ -]?token|secret|password)\s*[:=]|[a-zA-Z0-9_\-]{40,}", value, re.IGNORECASE):
        return fallback
    return value


def choice(value, options, fallback="unknown"):
    return value if isinstance(value, str) and value in options else fallback


def key_metadata(row):
    if row is None:
        return {"is_historical": True}
    protocol = str(row["protocol_type"] or "").strip().lower().replace("-", "_")
    protocol = {"openai": "openai_compat", "anthropic": "anthropic_native", "gemini": "gemini_native"}.get(protocol, protocol)
    model = row["model_slug"]
    binding = "request" if model is None or (isinstance(model, str) and not model.strip()) else "fixed" if model in PUBLIC_MODELS else "unlisted"
    return {"display_id": row["id"] if re.fullmatch(r"gk_[a-f0-9]{12}", str(row["id"])) else None,
            "upstream_provider": choice(row["upstream_provider"], {"openai", "claude"}),
            "protocol": choice(protocol, {"openai_compat", "anthropic_native", "gemini_native"}),
            "rotation": choice(row["rotation_strategy"], {"account_rotation", "aggregate_api_rotation", "hybrid_rotation", "hybrid_aggregate_first_rotation"}),
            "model_binding": binding, "bound_model": model if binding == "fixed" else None,
            "quota_limit_tokens": positive_int(row["quota_limit_tokens"]),
            "quota_config_known": row["quota_limit_tokens"] is None or (type(row["quota_limit_tokens"]) is int and abs(row["quota_limit_tokens"]) <= 2**53-1),
            "is_historical": False}


def nullable_bool(value):
    return bool(value) if type(value) is int and value in {0, 1} else None


def price_value(value):
    return value / 1_000_000 if type(value) is int and 0 <= value <= 2**53-1 else None


def collect_catalog(connection):
    catalog, unlisted = [], 0
    for row in connection.execute(CATALOG_SQL):
        if row["slug"] not in PUBLIC_MODELS:
            unlisted += 1
            continue
        price_status = choice(row["price_status"], {"official", "estimated", "custom", "missing"}, "missing")
        price = {"status": price_status}
        if price_status != "missing":
            for source, target in (("input", "input"), ("cached_input", "cached_input"), ("cache_write", "cache_write"), ("output", "output")):
                value = row[source + "_microusd_per_1m"]
                if source == "cache_write" and value is None:
                    value = row["input_microusd_per_1m"]
                price[target + "_usd_per_million"] = price_value(value)
        description = public_text(row["description"], maximum=600)
        catalog.append({"model": row["slug"], "name": public_text(row["display_name"], row["slug"]),
                        "description": DESCRIPTION_ZH.get(description, description),
                        "origin": choice(row["origin"], {"builtin", "custom"}),
                        "enabled": nullable_bool(row["enabled"]), "supported_in_api": nullable_bool(row["supported_in_api"]),
                        "instructions_mode": choice(row["instructions_mode"], {"passthrough", "fallback", "override"}),
                        "price": price, "route_count": row["route_count"],
                        "account_pool_routes": row["account_pool_count"], "aggregate_api_routes": row["aggregate_api_count"]})
    return catalog, unlisted
