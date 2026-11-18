"""Deterministic error signature classification and plain-Chinese diagnostic dictionary."""

import re

ERROR_LAYERS = ("gateway_pool", "upstream_openai", "member_policy", "client_cancelled")

SEVERITY_RANK = {"critical": 0, "warn": 1, "info": 2, "muted": 3}

LAYER_META = {
    "gateway_pool": {
        "label": "网关与共享号池",
        "owner": "团队运维 / 网关容量",
        "description": "中转站本地处理异常、3 账号共享池并发打满或账号触发短时冷却。",
    },
    "upstream_openai": {
        "label": "OpenAI 上游波动",
        "owner": "外部上游 / 跨境链路",
        "description": "OpenAI 官方模型算力繁忙、上游安全策略拦截或流式长连接抖动。",
    },
    "member_policy": {
        "label": "个人额度与配置",
        "owner": "成员侧 Key / 参数",
        "description": "个人滚动 7 天 $240 额度用尽、单 Key 并发上限、Key 缺失或模型名不匹配。",
    },
    "client_cancelled": {
        "label": "客户端主动取消",
        "owner": "正常交互中断（非故障）",
        "description": "成员点击 Stop 停止生成或本地断开连接（HTTP 499），系统净成功率已自动剔除。",
    },
}

CATALOG = [
    {
        "signature": "client_cancelled",
        "layer": "client_cancelled",
        "severity": "muted",
        "category": "client_cancelled",
        "title": "客户端主动取消或断开 (499)",
        "canonical": "客户端主动中断连接 (HTTP 499)",
        "meaning": "成员在 IDE 或终端点击了停止生成（Stop）、关闭了会话，或本地网络切换导致连接提前断开。",
        "action": "正常交互行为，并非中转站或上游故障；看板计算「系统净成功率」时已自动剔除此项。",
    },
    {
        "signature": "gateway_storage_error",
        "layer": "gateway_pool",
        "severity": "critical",
        "category": "upstream_error",
        "title": "中转站本地存储锁争用 (500)",
        "canonical": "网关本地读取用量锁超时 (database is locked)",
        "meaning": "高并发流式请求写库与准入额度查询争用本地 SQLite 锁导致失败（v25 已分离只读连接池修复）。",
        "action": "属于中转站自身故障信号；v25 上线后正常应保持为 0，若再出现需立即排查存储锁耗时的接口。",
    },
    {
        "signature": "pool_exhausted",
        "layer": "gateway_pool",
        "severity": "warn",
        "category": "no_available_account",
        "title": "共享号池并发打满或无可用候选 (503)",
        "canonical": "共享号池无空闲可用账号 (no_available_account)",
        "meaning": "3 个共享 GPT Pro 账号当前并发槽位已满（单号上限 5 路 / 总池 15 路），或候选账号正在短冷却恢复中。",
        "action": "非个人额度问题；等待 3~10 秒重试即可。若高峰期频繁发生，建议错峰或评估扩容共享 Pro 账号数。",
    },
    {
        "signature": "key_weekly_quota_exhausted",
        "layer": "member_policy",
        "severity": "warn",
        "category": "rate_limited",
        "title": "个人滚动 7 天 $240 额度已满 (429)",
        "canonical": "专属 Key 滚动 7 天额度已达 $240 上限",
        "meaning": "当前成员专属 Key 在过去 168 小时内的累计已计费消耗达到 $240.00 上限，网关准入层暂停放行。",
        "action": "无需更换模型或反复重试；在概览页「个人账户额度舱」查看下一笔历史消耗滑出 7 天窗口的释放时间。",
    },
    {
        "signature": "key_concurrency_limit",
        "layer": "member_policy",
        "severity": "info",
        "category": "rate_limited",
        "title": "触发单 Key 并发保护上限 (429)",
        "canonical": "当前专属 Key 并发请求数已达上限",
        "meaning": "同一专属 Key 已有请求正在生成（普通请求限单并发，AstraUltra 允许最多父请求+1子请求共 2 路）。",
        "action": "请等待上一条回复输出结束，或在客户端停止旧任务后再发起新请求。",
    },
    {
        "signature": "pool_cooldown",
        "layer": "gateway_pool",
        "severity": "warn",
        "category": "rate_limited",
        "title": "共享账号瞬时并发占用或短冷却 (429)",
        "canonical": "共享账号暂时不可用 / 进入短冷却 (account_temporarily_unavailable)",
        "meaning": "绑定的共享 Pro 账号正处于上游秒级速率冷却或瞬时 5 路并发上限，网关触发临时避让保护。",
        "action": "按 Retry-After 提示等待几秒后重试（通常 1~20 秒自动恢复），后续请求会路由至健康账号。",
    },
    {
        "signature": "key_disabled",
        "layer": "member_policy",
        "severity": "info",
        "category": "auth_error",
        "title": "专属 API Key 已被停用 (403)",
        "canonical": "专属 API Key 处于停用或归档状态",
        "meaning": "客户端携带的专属 Key 在网关后台已被禁用。",
        "action": "核对本地是否仍在使用旧测试 Key；如需恢复使用请联系团队管理员。",
    },
    {
        "signature": "auth_missing_or_invalid",
        "layer": "member_policy",
        "severity": "info",
        "category": "auth_error",
        "title": "API Key 缺失或客户端配置有误 (401/403)",
        "canonical": "请求未携带有效专属 API Key",
        "meaning": "客户端未传入 Authorization 密钥、本地 auth.json 未配置 OPENAI_API_KEY，或密钥复制不完整。",
        "action": "检查 Codex / IDE 配置与环境变量，确认已填入本人专属 Key 且无多余引号或空格。",
    },
    {
        "signature": "upstream_overloaded",
        "layer": "upstream_openai",
        "severity": "warn",
        "category": "upstream_overloaded",
        "title": "OpenAI 上游模型算力过载 (502)",
        "canonical": "OpenAI 官方模型容量繁忙 (server_is_overloaded)",
        "meaning": "OpenAI 官方对应模型集群当前算力紧张（Selected model is at capacity / server_is_overloaded），不扣个人额度。",
        "action": "可直接重试，或参考概览页「模型实时气象台」临时切换至当前状态为「顺畅」的其他模型。",
    },
    {
        "signature": "upstream_cyber_policy",
        "layer": "upstream_openai",
        "severity": "info",
        "category": "upstream_error",
        "title": "OpenAI 官方安全策略拦截 (502)",
        "canonical": "触发 OpenAI 上游安全策略拦截 (cyber_policy)",
        "meaning": "提示词触发了 OpenAI 官方的安全或网络安全风控规则（cyber_policy），并非中转站本地屏蔽。",
        "action": "可在提示词中补充合法研发/调试/性能分析语境，或拆解可能引发误报的代码片段后重试。",
    },
    {
        "signature": "upstream_timeout",
        "layer": "upstream_openai",
        "severity": "warn",
        "category": "timeout",
        "title": "上游高强度推理或首包超时 (504)",
        "canonical": "等待上游模型响应超时",
        "meaning": "超长上下文或 xhigh 高推理任务首字准备耗时超过单次网络等待阈值。",
        "action": "可重新提交请求，或适当精简单次附带的大日志/全仓库上下文后分段提问。",
    },
    {
        "signature": "upstream_stream_interrupted",
        "layer": "upstream_openai",
        "severity": "warn",
        "category": "network_error",
        "title": "上游流式吐字中途断开或重试耗尽 (502)",
        "canonical": "上游长连接或网络链路发生瞬时闪断",
        "meaning": "响应在流式传输过程中上游连接提前关闭、代理网络抖动，或自动重试预算已用尽。",
        "action": "属于网络/上游瞬时抖动，直接重试一次即可；极长输出任务可分步推进以降低断连概率。",
    },
    {
        "signature": "model_or_param_invalid",
        "layer": "member_policy",
        "severity": "info",
        "category": "invalid_request",
        "title": "请求模型未收录或参数不兼容 (400/404)",
        "canonical": "模型 ID 未收录或客户端请求参数无效",
        "meaning": "客户端请求了本 GPT 网关未开放的模型名（如 Claude 模型名）或传入了不被支持的参数格式。",
        "action": "前往「模型目录与气象」核对可用模型 Slug（如 gpt-6-astra、gpt-6.1-sol），Claude 模型请走专用入口。",
    },
    {
        "signature": "gateway_internal_error",
        "layer": "gateway_pool",
        "severity": "critical",
        "category": "upstream_error",
        "title": "中转站内部处理异常 (500)",
        "canonical": "中转站返回本地 HTTP 500 内部错误",
        "meaning": "请求在网关本地路由、编解码或落库阶段出现未预期异常。",
        "action": "属于中转站待迭代故障指标；需结合发生时间查看 codexmanager 容器日志排查修复。",
    },
    {
        "signature": "upstream_server_error",
        "layer": "upstream_openai",
        "severity": "warn",
        "category": "upstream_error",
        "title": "OpenAI 上游服务偶发内部错误 (502/503)",
        "canonical": "OpenAI 上游返回服务端内部错误 (server_error)",
        "meaning": "OpenAI 官方后端处理本次推理时发生偶发内部错误。",
        "action": "外部上游瞬时波动，通常直接重试一次即可恢复正常。",
    },
    {
        "signature": "unknown_error",
        "layer": "upstream_openai",
        "severity": "warn",
        "category": "unknown_error",
        "title": "其他未归类请求异常",
        "canonical": "未归类非 2xx 响应",
        "meaning": "请求未能返回 HTTP 2xx，错误特征暂未匹配预设诊断规则。",
        "action": "结合发生时间、状态码与模型定位根因，确认规律后可补充进诊断字典。",
    },
]

_CATALOG_BY_SIGNATURE = {item["signature"]: item for item in CATALOG}

_KIND_RE = re.compile(r"\bkind=([a-z0-9_]{1,40})\b", re.IGNORECASE)
_CODE_RE = re.compile(r"\bcode=([a-z0-9_]{1,40})\b", re.IGNORECASE)
_RETRY_RE = re.compile(r"\bretry_after=([0-9]{1,4}(?:\.[0-9]+)?s?)\b", re.IGNORECASE)
_QUOTA_RE = re.compile(r"\bused\s+(\d{1,8})\s+cents,\s*limit\s+(\d{1,8})\s+cents\b", re.IGNORECASE)
_ATTEMPTED_RE = re.compile(r"\battempted=([^;\s]+)", re.IGNORECASE)
_LAST_ATTEMPT_RE = re.compile(
    r"\blast_attempt=generation retry budget exhausted:\s*(shared pool limit|request send limit)\b",
    re.IGNORECASE,
)


def _match_signature(status_code, text: str) -> str:
    code = status_code if type(status_code) is int and 100 <= status_code <= 599 else None

    if code == 499 or any(m in text for m in (
        "broken pipe", "client disconnected", "client cancelled", "client canceled",
        "downstream disconnect", "response write failed: broken pipe",
        "客户端取消", "客户端中断",
    )):
        return "client_cancelled"

    if code == 500 and any(m in text for m in (
        "database is locked", "读取用量失败", "sqlite", "storage error", "pool timed out",
    )):
        return "gateway_storage_error"

    if any(m in text for m in (
        "no available account", "no_available_account", "无可用账号",
        "account pool exhausted", "no healthy account",
    )):
        return "pool_exhausted"

    if any(m in text for m in (
        "weekly quota exhausted", "周额度已用尽", "quota limit exceeded",
    )):
        return "key_weekly_quota_exhausted"

    if code == 429 and "account temporarily unavailable" not in text and any(m in text for m in (
        "单并发", "per-key", "per key", "key concurrency", "max concurrent",
        "concurrent request", "active request",
    )):
        return "key_concurrency_limit"

    if code == 429 or any(m in text for m in (
        "account temporarily unavailable", "account_pool_cooldown", "bound_account_cooldown",
        "bound_account_inflight", "bound_account_unavailable",
        "rate limit", "rate_limited", "too many requests", "限流", "账号暂时不可用",
    )):
        return "pool_cooldown"

    if code in {401, 403} and any(m in text for m in ("已禁用", "api key disabled", "key disabled")):
        return "key_disabled"

    if code in {401, 403} or any(m in text for m in (
        "missing api key", "invalid api key", "unauthorized", "forbidden",
        "缺少 api key", "未配置auth.json", "鉴权失败", "认证失败",
    )):
        return "auth_missing_or_invalid"

    if any(m in text for m in (
        "server_is_overloaded", "at capacity", "overloaded", "over capacity", "服务繁忙", "上游繁忙",
    )):
        return "upstream_overloaded"

    if any(m in text for m in (
        "cyber_policy", "cybersecurity", "content_policy", "usage policy", "safety_system",
    )):
        return "upstream_cyber_policy"

    if code in {408, 504} or any(m in text for m in (
        "timeout", "timed out", "deadline exceeded", "超时",
    )):
        return "upstream_timeout"

    if any(m in text for m in (
        "连接中断", "upstream stream read failed", "upstream response did not complete",
        "response write failed", "transport error", "error decoding response body",
        "error sending request", "connection reset", "connection refused",
        "unexpected eof", "incomplete utf-8", "utf8 error", "network error",
        "dns error", "generation retry budget exhausted", "网络错误", "连接失败",
    )):
        return "upstream_stream_interrupted"

    if (code is not None and code in {400, 404, 405, 413, 415, 422}) or any(m in text for m in (
        "模型不在模型列表中", "model_not_found", "unsupported model", "invalid_request", "invalid request",
    )):
        return "model_or_param_invalid"

    if code == 500 and "code=server_error" not in text and "upstream" not in text:
        return "gateway_internal_error"

    if (code is not None and 500 <= code <= 599) or "code=server_error" in text:
        return "upstream_server_error"

    return "unknown_error"


def classify_failure(status_code, error) -> dict:
    """Return full deterministic diagnostic metadata for a non-2xx request_logs entry."""
    text = str(error or "").casefold()
    signature = _match_signature(status_code, text)
    return dict(_CATALOG_BY_SIGNATURE[signature])


def safe_failure_message(status_code, error, diag=None) -> str:
    """Build a strictly allowlisted summary without echoing raw DB errors, emails, URLs, or secrets."""
    info = diag if diag is not None else classify_failure(status_code, error)
    code = status_code if type(status_code) is int and 100 <= status_code <= 599 else None
    raw = str(error or "")
    parts = [f"HTTP {code}" if code is not None else "HTTP —", info["canonical"]]

    kind_match = _KIND_RE.search(raw)
    if kind_match:
        parts.append(f"kind={kind_match.group(1).lower()}")

    code_match = _CODE_RE.search(raw)
    if code_match:
        code_tok = code_match.group(1).lower()
        if not kind_match or code_tok != kind_match.group(1).lower():
            parts.append(f"code={code_tok}")

    retry_match = _RETRY_RE.search(raw)
    if retry_match:
        parts.append(f"retry_after={retry_match.group(1).lower()}")

    quota_match = _QUOTA_RE.search(raw)
    if quota_match:
        used_c = int(quota_match.group(1))
        limit_c = int(quota_match.group(2))
        parts.append(f"已用 ${used_c / 100:.2f} / ${limit_c / 100:.2f}")

    attempted_match = _ATTEMPTED_RE.search(raw)
    if attempted_match:
        count = sum(1 for item in attempted_match.group(1).split(",") if item.strip())
        if count > 0:
            parts.append(f"已尝试 {count} 个共享账号")

    budget_match = _LAST_ATTEMPT_RE.search(raw)
    if budget_match:
        parts.append(budget_match.group(1).lower())

    return " · ".join(parts)[:310]


def public_catalog_entries() -> list[dict]:
    """Return copy of diagnostic reference catalog for API or tests."""
    return [dict(item) for item in CATALOG]
