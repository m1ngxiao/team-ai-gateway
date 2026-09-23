from typing import Literal

from pydantic import BaseModel, ConfigDict, Field


class ClosedModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True, allow_inf_nan=False)


class Usage(ClosedModel):
    requests: int = Field(ge=0)
    input_tokens: int = Field(ge=0)
    cached_tokens: int = Field(ge=0)
    output_tokens: int = Field(ge=0)
    total_tokens: int = Field(ge=0)
    estimated_usd: float = Field(ge=0)


class Periods(ClosedModel):
    today: Usage
    week: Usage
    recorded: Usage


class PoolUsage(ClosedModel):
    openai: Periods
    claude: Periods
    unattributed: Periods


class Window(ClosedModel):
    minutes: int | None = Field(default=None, gt=0)
    remaining_percent: float | None = Field(default=None, ge=0, le=100)
    resets_at: int | None = Field(default=None, ge=0)


class Account(ClosedModel):
    id: str = Field(pattern=r"^acct-[a-f0-9]{12}$")
    label: str = Field(min_length=1, max_length=32)
    group_name: str | None = Field(default=None, min_length=1, max_length=128)
    plan: Literal["pro", "plus", "team", "business", "enterprise", "free", "unknown"]
    status: Literal["enabled", "disabled", "unavailable", "unknown"]
    primary: Window
    secondary: Window
    captured_at: int | None = Field(default=None, ge=0)
    email: str | None = Field(default=None, max_length=254, pattern=r"^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,63}$")
    subscription_expires_at: int | None = Field(default=None, gt=0)
    subscription_renews_at: int | None = Field(default=None, gt=0)
    sort_order: int | None = None
    proxy_state: Literal["configured", "disabled", "inherited", "unknown"] = "unknown"
    capacity_override: bool = False
    capacity_primary_tokens: int | None = Field(default=None, gt=0)
    capacity_secondary_tokens: int | None = Field(default=None, gt=0)
    spark_primary: Window = Field(default_factory=Window)
    spark_secondary: Window = Field(default_factory=Window)
    reset_available_count: int | None = Field(default=None, ge=0, le=9007199254740991)
    reset_next_expires_at: int | None = Field(default=None, gt=0)


class ClaudeAccount(ClosedModel):
    id: str = Field(pattern=r"^acct-[a-f0-9]{12}$")
    label: str = Field(min_length=1, max_length=32)
    email: str | None = Field(default=None, max_length=254, pattern=r"^[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,63}$")
    plan: Literal["pro", "max", "team", "enterprise", "free", "unknown"] = "unknown"
    status: Literal["enabled", "disabled", "needs_login", "unknown"] = "unknown"
    sort_order: int | None = None
    updated_at: int | None = Field(default=None, ge=0)


class Key(ClosedModel):
    id: str = Field(pattern=r"^key-[a-f0-9]{12}$")
    label: str = Field(min_length=1, max_length=128)
    group_name: str | None = Field(default=None, min_length=1, max_length=128)
    status: Literal["enabled", "disabled", "unavailable", "unknown"]
    last_used_at: int | None = Field(default=None, ge=0)
    usage: Periods
    display_id: str | None = Field(default=None, pattern=r"^gk_[a-f0-9]{12}$")
    protocol: Literal["openai_compat", "anthropic_native", "gemini_native", "unknown"] = "unknown"
    rotation: Literal["account_rotation", "aggregate_api_rotation", "hybrid_rotation", "hybrid_aggregate_first_rotation", "unknown"] = "unknown"
    model_binding: Literal["request", "fixed", "unlisted", "unknown"] = "unknown"
    bound_model: str | None = Field(default=None, pattern=r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$")
    quota_limit_tokens: int | None = Field(default=None, gt=0, le=9007199254740991)
    quota_config_known: bool = False
    is_historical: bool = False
    upstream_provider: Literal["openai", "claude", "unknown"] = "unknown"


class KeyGroup(ClosedModel):
    id: str = Field(pattern=r"^group-[a-f0-9]{12}$")
    label: str = Field(min_length=1, max_length=128)
    kind: Literal["named", "unrestricted", "unattributed"]
    key_count: int = Field(ge=0, le=5000)
    usage: Periods


class ModelPrice(ClosedModel):
    status: Literal["official", "estimated", "custom", "missing", "unknown"] = "missing"
    input_usd_per_million: float | None = Field(default=None, ge=0)
    cached_input_usd_per_million: float | None = Field(default=None, ge=0)
    cache_write_usd_per_million: float | None = Field(default=None, ge=0)
    output_usd_per_million: float | None = Field(default=None, ge=0)


class CatalogModel(ClosedModel):
    model: str = Field(pattern=r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$")
    name: str = Field(min_length=1, max_length=128)
    description: str = Field(default="", max_length=600)
    origin: Literal["builtin", "custom", "unknown"] = "unknown"
    enabled: bool | None = None
    supported_in_api: bool | None = None
    instructions_mode: Literal["passthrough", "fallback", "override", "unknown"] = "unknown"
    price: ModelPrice = Field(default_factory=ModelPrice)
    route_count: int = Field(default=0, ge=0)
    account_pool_routes: int = Field(default=0, ge=0)
    aggregate_api_routes: int = Field(default=0, ge=0)


class ModelUsage(ClosedModel):
    model: str = Field(pattern=r"^[a-zA-Z0-9][a-zA-Z0-9._-]{0,63}$")
    usage: Usage


class Snapshot(ClosedModel):
    schema_version: Literal[1]
    generated_at: int = Field(ge=0)
    timezone: Literal["Asia/Shanghai"]
    accounts: list[Account] = Field(max_length=1000)
    claude_accounts: list[ClaudeAccount] = Field(default_factory=list, max_length=1000)
    keys: list[Key] = Field(max_length=5000)
    key_groups: list[KeyGroup] = Field(default_factory=list, max_length=5001)
    totals: Periods
    pool_usage: PoolUsage | None = None
    models_week: list[ModelUsage] = Field(max_length=50)
    model_catalog: list[CatalogModel] = Field(default_factory=list, max_length=500)
    catalog_unlisted_count: int = Field(default=0, ge=0)
