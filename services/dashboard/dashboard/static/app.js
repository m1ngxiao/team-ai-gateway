"use strict";

/* --------------------------------------------------------------------------
 * Team AI Gate · Precision Editorial Observatory (CSP-Strict, Zero-innerHTML)
 * -------------------------------------------------------------------------- */

const STORAGE_MEMBER_KEY = "stats_selected_member_key_id";
const STORAGE_ERROR_LAYER = "stats_selected_error_layer";
const DEFAULT_TERMINAL_ORIGINS = {
  public: { label: "公网入口 (api.example.com)", url: "https://api.example.com" },
  intranet: { label: "内网直连 (127.0.0.1:48760)", url: "http://127.0.0.1:48760" },
};
function resolveTerminalOrigins() {
  const cfg = snapshot && snapshot.terminal_origins;
  if (cfg && cfg.public && cfg.public.url) {
    return {
      public: cfg.public,
      intranet: cfg.intranet && cfg.intranet.url ? cfg.intranet : DEFAULT_TERMINAL_ORIGINS.intranet,
    };
  }
  return DEFAULT_TERMINAL_ORIGINS;
}
let terminalOriginMode = "public";

const VIEW_META = {
  overview: { eyebrow: "OBSERVATORY · COMMAND DECK", title: "团队调度与健康概览" },
  errors: { eyebrow: "PILLAR 03 · DAILY ERROR DIAGNOSTICS", title: "中转站每日报错与含义诊断" },
  keys: { eyebrow: "PILLAR 01 · MEMBER QUOTAS", title: "成员专属 Key 与 $240 额度" },
  accounts: { eyebrow: "PILLAR 02 · SHARED GPT POOL", title: "GPT 共享 Pro 20X 账号池" },
  usage: { eyebrow: "CONSUMPTION & GROUPS", title: "团队用量、分组与模型分布" },
  models: { eyebrow: "MODEL WEATHER & CATALOG", title: "模型实时气象与价格目录" },
  speed: { eyebrow: "INFERENCE TELEMETRY & BENCHMARK", title: "历史推理速度与在线测速" },
};

const LAYER_META = {
  gateway_pool: { label: "网关与共享号池", owner: "网关 / 容量迭代", tone: "rose", desc: "本地处理异常、3 号池并发打满或共享账号短冷却" },
  upstream_openai: { label: "OpenAI 上游波动", owner: "外部上游 / 网络", tone: "amber", desc: "官方模型容量繁忙、安全风控拦截或跨境流中断" },
  member_policy: { label: "个人额度与配置", owner: "成员 Key / 参数", tone: "sky", desc: "$240 滚动周额度满、单并发或 Key / 模型名配置错误" },
  client_cancelled: { label: "客户端主动取消", owner: "正常手动停止", tone: "muted", desc: "IDE 点击 Stop 或本地断连（499），净成功率已剔除" },
};

const WEATHER_META = {
  smooth: { label: "顺畅推荐", tone: "emerald" },
  busy: { label: "偶发繁忙", tone: "amber" },
  degraded: { label: "官方过载", tone: "rose" },
  idle: { label: "近期空闲", tone: "muted" },
};

const ERROR_DICTIONARY_REF = [
  { signature: "gateway_storage_error", layer: "gateway_pool", severity: "critical", codes: [500],
    title: "中转站本地存储锁争用 (500)",
    meaning: "高并发流式写库与准入额度查询争用 SQLite 锁导致失败（v25 已分离只读池修复）。",
    action: "属于网关自身 Bug 信号；v25 上线后正常应保持 0 次，若复现需立刻排查慢 SQL。" },
  { signature: "pool_exhausted", layer: "gateway_pool", severity: "warn", codes: [503],
    title: "共享号池并发打满或无可用候选 (503)",
    meaning: "3 个共享 GPT Pro 账号当前并发槽位已满（单号上限 5 路 / 总池 15 路），或候选均在短冷却恢复中。",
    action: "非个人额度问题，等待 3~10 秒直接重试；若高峰频繁触发则需考虑错峰或扩容账号。" },
  { signature: "pool_cooldown", layer: "gateway_pool", severity: "warn", codes: [429],
    title: "共享账号瞬时并发占用或短冷却 (429)",
    meaning: "绑定共享账号正处于上游秒级速率冷却或瞬时并发占满，网关触发短时避让保护。",
    action: "按 Retry-After 提示等待几秒后重试，系统会自动切换其余可用账号。" },
  { signature: "upstream_overloaded", layer: "upstream_openai", severity: "warn", codes: [502],
    title: "OpenAI 上游模型算力过载 (502)",
    meaning: "OpenAI 官方对应模型集群当前算力满载（server_is_overloaded / at capacity），不扣个人额度。",
    action: "直接重试，或根据概览「模型实时气象台」切换至「顺畅推荐」模型。" },
  { signature: "upstream_stream_interrupted", layer: "upstream_openai", severity: "warn", codes: [502],
    title: "上游流式吐字中途断开或重试耗尽 (502)",
    meaning: "响应流式输出过程中上游连接闪断、字节流未完整结束或代理网络抖动。",
    action: "偶发链路波动，直接重试即可；极长上下文建议拆分为多轮小步执行。" },
  { signature: "upstream_timeout", layer: "upstream_openai", severity: "warn", codes: [504, 408],
    title: "上游高强度推理或首包超时 (504)",
    meaning: "超长上下文或 xhigh 深度推理耗时超过单次网关连接等待上限。",
    action: "重新提交请求，或裁剪冗余长日志后重试。" },
  { signature: "upstream_cyber_policy", layer: "upstream_openai", severity: "info", codes: [502],
    title: "OpenAI 官方安全策略拦截 (502)",
    meaning: "提示词内容触发 OpenAI 官方网络安全或内容策略风控（cyber_policy），非中转站限制。",
    action: "在提示词中补充合法研发/调试语境或拆解敏感表述后重试。" },
  { signature: "upstream_server_error", layer: "upstream_openai", severity: "warn", codes: [502, 503],
    title: "OpenAI 上游服务偶发内部错误 (502/503)",
    meaning: "OpenAI 官方后端处理请求时出现偶发 server_error。",
    action: "外部瞬时波动，通常立即重试一次即可恢复。" },
  { signature: "key_weekly_quota_exhausted", layer: "member_policy", severity: "warn", codes: [429],
    title: "个人滚动 7 天 $240 额度已满 (429)",
    meaning: "成员专属 Key 过去 168 小时已计费消耗达到 $240.00 上限。",
    action: "在概览「个人账户额度舱」查看下一笔账单滑出 7 天窗口的精确释放时间与金额。" },
  { signature: "key_concurrency_limit", layer: "member_policy", severity: "info", codes: [429],
    title: "触发单 Key 并发保护上限 (429)",
    meaning: "同一专属 Key 已有请求在执行（普通 1 并发；AstraUltra 允许父+1子共 2 并发）。",
    action: "等待上一条请求返回完毕或先停止旧任务再发送新请求。" },
  { signature: "auth_missing_or_invalid", layer: "member_policy", severity: "info", codes: [401, 403],
    title: "API Key 缺失或客户端配置有误 (401/403)",
    meaning: "客户端未发送有效 Authorization 头或本地 auth.json 未正确配置 OPENAI_API_KEY。",
    action: "检查本地环境变量与 auth.json，确认填入本人专属 Key 且没有多余字符。" },
  { signature: "key_disabled", layer: "member_policy", severity: "info", codes: [403],
    title: "专属 API Key 已被停用 (403)",
    meaning: "所使用的 API Key 在网关后台处于禁用/归档状态。",
    action: "核对是否误用了旧 Key；如需启用请联系团队管理员。" },
  { signature: "model_or_param_invalid", layer: "member_policy", severity: "info", codes: [400, 404],
    title: "请求模型未收录或参数不兼容 (400/404)",
    meaning: "传入了本 GPT 网关未收录的模型 ID（如 Claude 模型名）或不支持的参数。",
    action: "在「模型气象与价格」查看可用模型 Slug，Claude 模型请切至专用入口。" },
  { signature: "gateway_internal_error", layer: "gateway_pool", severity: "critical", codes: [500],
    title: "中转站内部处理异常 (500)",
    meaning: "请求在网关本地路由或响应包装阶段返回了未预期 500 错误。",
    action: "属于待修复网关故障，需结合发生时间排查网关容器日志。" },
  { signature: "client_cancelled", layer: "client_cancelled", severity: "muted", codes: [499],
    title: "客户端主动取消或断开 (499)",
    meaning: "用户在 IDE 中主动点击 Stop 停止生成或本地断开连接。",
    action: "正常交互中断，不属于故障；「系统净成功率」已自动剔除。" },
  { signature: "unknown_error", layer: "upstream_openai", severity: "warn", codes: [],
    title: "其他未归类请求异常",
    meaning: "暂未匹配已知预设规则的非 2xx 响应。",
    action: "结合时间与状态码排查并沉淀至诊断字典。" },
];

const NAV_ICONS = {
  overview: "M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z",
  errors: "M12 3L2 20h20L12 3zm0 6v5m0 3.5v.5",
  keys: "M15 8a4 4 0 1 1-7.5 2H3v3h2v2h2v2h3v-3.5a4 4 0 0 1 5-5.5z",
  accounts: "M4 7h16v10H4zM8 11h3m2 0h3",
  usage: "M4 19h16M7 16V9m5 7V5m5 11v-4",
  models: "M12 3l8 4.5v9L12 21l-8-4.5v-9L12 3zm0 0v9m8-4.5l-8 4.5-8-4.5",
  speed: "M13 3L5 14h6l-1 7 9-12h-6l0-6z",
};

const VALID_VIEWS = new Set(Object.keys(VIEW_META));

let snapshot = null;
let liveState = null;
let csrfToken = null;
let authGeneration = 0;
let activeView = "overview";
let activePeriod = "week";
let selectedKeyId = "";
let selectedErrorDate = ""; // "" => latest/today; "week" => 7d aggregate
let overviewErrorTab = "today"; // "today" | "yesterday" | "week"
let selectedLayerFilter = "all";
let onlySelectedMemberFailures = false;
let liveTimer = null;
let refreshTimer = null;
let pendingSpeedtest = false;

/* ---------- Safe DOM & Format Helpers ---------- */

function byId(id) {
  return document.getElementById(id);
}

function el(tag, className, ...children) {
  const node = document.createElement(tag);
  if (className) node.setAttribute("class", className);
  for (const child of children) {
    if (child == null || child === false) continue;
    node.append(typeof child === "string" ? child : child);
  }
  return node;
}

function svgEl(tag, attrs, ...children) {
  const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
  if (attrs) {
    for (const [k, v] of Object.entries(attrs)) {
      if (v !== undefined && v !== null) node.setAttribute(k, String(v));
    }
  }
  for (const child of children) {
    if (child != null) node.append(child);
  }
  return node;
}

function readStorage(key, fallback = "") {
  try {
    return typeof localStorage !== "undefined" ? (localStorage.getItem(key) || fallback) : fallback;
  } catch {
    return fallback;
  }
}

function writeStorage(key, value) {
  try {
    if (typeof localStorage !== "undefined") {
      if (value) localStorage.setItem(key, value);
      else localStorage.removeItem(key);
    }
  } catch {
    // Ignore storage quota / privacy mode errors cleanly.
  }
}

function fmtInt(value) {
  if (value === null || value === undefined || !Number.isFinite(Number(value))) return "—";
  return Math.round(Number(value)).toLocaleString("en-US");
}

function fmtTokens(value) {
  if (value === null || value === undefined || !Number.isFinite(Number(value))) return "—";
  const n = Number(value);
  if (n >= 1e9) return (n / 1e9).toFixed(2) + "B";
  if (n >= 1e6) return (n / 1e6).toFixed(2) + "M";
  if (n >= 1e4) return (n / 1e3).toFixed(1) + "K";
  return Math.round(n).toLocaleString("en-US");
}

function fmtUsd(value) {
  if (value === null || value === undefined || !Number.isFinite(Number(value))) return "—";
  return "$" + Number(value).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
}

function fmtCents(cents) {
  if (cents === null || cents === undefined || !Number.isFinite(Number(cents))) return "—";
  return fmtUsd(Number(cents) / 100);
}

function fmtPct(value, digits = 1) {
  if (value === null || value === undefined || !Number.isFinite(Number(value))) return "—";
  return Number(value).toFixed(digits) + "%";
}

function fmtTime(epoch) {
  if (!epoch || !Number.isFinite(Number(epoch))) return "—";
  const d = new Date(Number(epoch) * 1000);
  const parts = new Intl.DateTimeFormat("zh-CN", {
    timeZone: "Asia/Shanghai",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).formatToParts(d);
  const map = Object.fromEntries(parts.map((p) => [p.type, p.value]));
  return `${map.month}/${map.day} ${map.hour}:${map.minute}`;
}

function fmtDuration(ms) {
  if (ms === null || ms === undefined || !Number.isFinite(Number(ms))) return "—";
  const v = Number(ms);
  return v >= 1000 ? (v / 1000).toFixed(2) + "s" : Math.round(v) + "ms";
}

function fmtCountdown(targetEpoch, nowSec) {
  if (!targetEpoch || !Number.isFinite(Number(targetEpoch))) return "—";
  const diff = Number(targetEpoch) - nowSec;
  if (diff <= 0) return "等待刷新重置";
  const days = Math.floor(diff / 86400);
  const hours = Math.floor((diff % 86400) / 3600);
  const mins = Math.floor((diff % 3600) / 60);
  if (days > 0) return `${days}天 ${hours}小时`;
  if (hours > 0) return `${hours}小时 ${mins}分`;
  return `${Math.max(1, mins)}分钟`;
}

function fmtRelativeAge(epoch, nowSec) {
  if (!epoch || !Number.isFinite(Number(epoch))) return "暂无记录";
  const diff = Math.max(0, nowSec - Number(epoch));
  if (diff < 60) return "刚刚活跃";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}

function fmtModelPrice(priceValue) {
  if (priceValue === null || priceValue === undefined || !Number.isFinite(Number(priceValue))) return "未配置";
  const num = Number(priceValue);
  if (num === 0) return "$0.00";
  return num >= 1 ? "$" + num.toFixed(2) : "$" + num.toFixed(3);
}

function clamp(val, lo, hi) {
  return Math.max(lo, Math.min(hi, val));
}

function quotaTone(remainingPct) {
  if (remainingPct === null || remainingPct === undefined) return "muted";
  if (remainingPct <= 12) return "rose";
  if (remainingPct <= 35) return "amber";
  return "emerald";
}

function buildLinearMeter(percent, toneClass) {
  const pct = clamp(Number.isFinite(Number(percent)) ? Number(percent) : 0, 0, 100);
  const fillW = Math.round((pct / 100) * 240 * 10) / 10;
  return svgEl(
    "svg",
    { class: "meter-svg", viewBox: "0 0 240 8", preserveAspectRatio: "none", "aria-hidden": "true" },
    svgEl("rect", { class: "meter-track", x: 0, y: 0, width: 240, height: 8, rx: 4 }),
    svgEl("rect", { class: `meter-fill tone-${toneClass}`, x: 0, y: 0, width: fillW, height: 8, rx: 4 })
  );
}

function buildCircleRing(percent, toneClass, centerTop, centerBottom) {
  const radius = 50;
  const circum = 2 * Math.PI * radius;
  const pct = clamp(Number.isFinite(Number(percent)) ? Number(percent) : 0, 0, 100);
  const offset = circum * (1 - pct / 100);
  const ring = svgEl(
    "svg",
    { class: "ring-svg", viewBox: "0 0 120 120", "aria-hidden": "true" },
    svgEl("circle", { class: "ring-track", cx: 60, cy: 60, r: radius }),
    svgEl("circle", {
      class: `ring-fill tone-${toneClass}`,
      cx: 60,
      cy: 60,
      r: radius,
      "stroke-dasharray": circum.toFixed(2),
      "stroke-dashoffset": offset.toFixed(2),
    })
  );
  const center = el(
    "div",
    "ring-center",
    el("span", "ring-pct num", centerTop),
    el("span", "ring-sub", centerBottom)
  );
  return el("div", "ring-wrap", ring, center);
}

function buildSlotPips(activeCount, capacity = 5) {
  const count = clamp(Number(activeCount) || 0, 0, capacity);
  const pips = el("span", "slot-pips");
  for (let i = 0; i < capacity; i += 1) {
    pips.append(el("span", i < count ? "slot-pip is-active" : "slot-pip"));
  }
  return pips;
}

function currentKeys() {
  return (snapshot && Array.isArray(snapshot.keys) ? snapshot.keys : []).filter((k) => !k.is_historical);
}

function findSelectedKey() {
  const keys = currentKeys();
  return keys.find((k) => k.id === selectedKeyId) || null;
}

function accountStatusMeta(status) {
  if (status === "enabled") return { label: "在线正常", tone: "emerald" };
  if (status === "disabled") return { label: "已禁用", tone: "muted" };
  if (status === "unavailable") return { label: "暂不可用", tone: "rose" };
  return { label: "状态未知", tone: "muted" };
}

function renderNavIcons() {
  for (const holder of document.querySelectorAll("[data-icon]")) {
    const key = holder.dataset.icon;
    const d = NAV_ICONS[key];
    if (!d) continue;
    holder.replaceChildren(
      svgEl(
        "svg",
        { viewBox: "0 0 24 24", width: 17, height: 17, fill: "none", stroke: "currentColor", "stroke-width": 1.75, "stroke-linecap": "round", "stroke-linejoin": "round" },
        svgEl("path", { d })
      )
    );
  }
}

/* ---------- Navigation & Member Perspective ---------- */

function setView(viewName, { pushHash = true } = {}) {
  const target = VALID_VIEWS.has(viewName) ? viewName : "overview";
  activeView = target;
  for (const btn of document.querySelectorAll("[data-view]")) {
    const match = btn.dataset.view === target;
    btn.setAttribute("class", match ? "nav-item is-active" : "nav-item");
  }
  for (const key of VALID_VIEWS) {
    const section = byId(`view-${key}`);
    if (section) section.hidden = key !== target;
  }
  const meta = VIEW_META[target];
  byId("topbar-eyebrow").textContent = meta.eyebrow;
  byId("topbar-title").textContent = meta.title;
  if (pushHash && location.hash !== `#${target}`) {
    history.replaceState(null, "", `#${target}`);
  }
  if (snapshot) renderActiveView();
}

function syncMemberSelectors() {
  const keys = currentKeys().slice().sort((a, b) => {
    if (a.status === "enabled" && b.status !== "enabled") return -1;
    if (a.status !== "enabled" && b.status === "enabled") return 1;
    return (b.weekly_used_cents || 0) - (a.weekly_used_cents || 0) || a.label.localeCompare(b.label, "zh-CN");
  });

  if (selectedKeyId && !keys.some((k) => k.id === selectedKeyId)) {
    selectedKeyId = "";
    writeStorage(STORAGE_MEMBER_KEY, "");
  }

  for (const selectId of ["global-member-select", "overview-member-select"]) {
    const select = byId(selectId);
    if (!select) continue;
    const placeholder = selectId === "global-member-select"
      ? "全队公共视角（请选择姓名）"
      : "切换个人视角";
    const opts = [el("option", "", placeholder)];
    opts[0].value = "";
    for (const k of keys) {
      const rem = fmtCents(k.weekly_remaining_cents ?? 24000);
      const label = k.status === "disabled" ? `${k.label} (已停用)` : `${k.label} · 余 ${rem}`;
      const opt = el("option", "", label);
      opt.value = k.id;
      opts.push(opt);
    }
    select.replaceChildren(...opts);
    select.value = selectedKeyId;
  }
}

function selectMember(keyId) {
  selectedKeyId = keyId || "";
  writeStorage(STORAGE_MEMBER_KEY, selectedKeyId);
  const g = byId("global-member-select");
  const o = byId("overview-member-select");
  if (g) g.value = selectedKeyId;
  if (o) o.value = selectedKeyId;
  if (!selectedKeyId) onlySelectedMemberFailures = false;
  if (snapshot) {
    renderPersonalCabin();
    if (activeView === "errors") renderErrorsView();
    if (activeView === "keys") renderKeysView();
  }
}

/* ---------- Render: Pillar 1 (Personal Quota Deck) ---------- */

function personalErrorCount(key) {
  const failures = (snapshot && snapshot.recent_failures) || [];
  let nonCancelWeek = 0;
  const todayDate = latestHealthDate();
  let todayCount = 0;
  for (const item of failures) {
    if (item.key_id !== key.id && item.key_label !== key.label) continue;
    if (item.layer === "client_cancelled") continue;
    nonCancelWeek += 1;
    if (todayDate && item.date === todayDate) todayCount += 1;
  }
  return { todayCount, nonCancelWeek };
}

function renderPersonalCabin() {
  const body = byId("personal-cabin-body");
  const pill = byId("personal-status-pill");
  const nowSec = Math.floor(Date.now() / 1000);
  const selected = findSelectedKey();
  const keys = currentKeys();

  if (!selected) {
    pill.textContent = `全队 ${keys.filter((k) => k.status === "enabled").length} 位启用中`;
    pill.setAttribute("class", "status-pill tone-sky");

    const topActive = keys
      .filter((k) => k.status === "enabled")
      .slice()
      .sort((a, b) => (b.weekly_used_cents || 0) - (a.weekly_used_cents || 0) || (b.usage?.today?.requests || 0) - (a.usage?.today?.requests || 0))
      .slice(0, 10);

    const chips = el("div", "chip-cloud");
    for (const item of topActive) {
      const rem = fmtCents(item.weekly_remaining_cents ?? 24000);
      const chip = el("button", "member-chip", item.label, el("span", "member-chip-amt", rem));
      chip.type = "button";
      chip.addEventListener("click", () => selectMember(item.id));
      chips.append(chip);
    }

    const totalTeamUsedToday = snapshot?.totals?.today?.estimated_usd || 0;
    const maxMemberUsed = keys.reduce((mx, k) => Math.max(mx, k.weekly_used_cents || 0), 0);
    const healthyCount = keys.filter((k) => k.status === "enabled" && (k.weekly_remaining_cents ?? 24000) > 2400).length;

    const summaryStats = el(
      "div",
      "personal-stats-grid",
      el("div", "stat-tile", el("span", "stat-tile-label", "全队个人周上限"), el("span", "stat-tile-val num", "$240.00"), el("span", "stat-tile-sub", "滚动 168 小时释放")),
      el("div", "stat-tile", el("span", "stat-tile-label", "成员余量充足"), el("span", "stat-tile-val num", `${healthyCount} / ${keys.length}`), el("span", "stat-tile-sub", `最高已用 ${fmtCents(maxMemberUsed)}`)),
      el("div", "stat-tile", el("span", "stat-tile-label", "团队今日参考消耗"), el("span", "stat-tile-val num", fmtUsd(totalTeamUsedToday)), el("span", "stat-tile-sub", "点击姓名秒查个人账单"))
    );

    const cta = el("button", "btn subtle small", "终端怎么实时查余量 →");
    cta.id = "personal-terminal-cta";
    cta.type = "button";
    cta.addEventListener("click", openTerminalModal);

    body.replaceChildren(
      el(
        "div",
        "onboarding-box",
        el(
          "div",
          "onboarding-copy",
          el("h3", "", "点击你的名字，锁定专属 $240 滚动额度"),
          el("p", "", "选择后浏览器会自动记住你的视角；随时可核对个人剩余额度、下一笔历史消费何时释放以及专属报错详情。")
        ),
        chips.childNodes.length ? chips : el("p", "muted", "暂无活跃成员 Key 记录。"),
        summaryStats,
        el("div", "personal-cta-row", cta)
      )
    );
    return;
  }

  const limitCents = selected.weekly_limit_cents || 24000;
  const usedCents = clamp(selected.weekly_used_cents || 0, 0, limitCents);
  const remCents = selected.weekly_remaining_cents ?? Math.max(0, limitCents - usedCents);
  const remPct = clamp((remCents / limitCents) * 100, 0, 100);
  const usedPct = clamp((usedCents / limitCents) * 100, 0, 100);
  const tone = selected.status === "disabled" ? "rose" : remCents === 0 ? "rose" : remPct <= 25 ? "amber" : "emerald";

  if (selected.status === "disabled") {
    pill.textContent = "Key 已停用";
    pill.setAttribute("class", "status-pill tone-rose");
  } else if (remCents === 0) {
    pill.textContent = "周额度已用尽 · 等待释放";
    pill.setAttribute("class", "status-pill tone-rose");
  } else if (remPct <= 25) {
    pill.textContent = "余量偏低 · 请关注释放";
    pill.setAttribute("class", "status-pill tone-amber");
  } else {
    pill.textContent = "准入畅通 · 可立即调用";
    pill.setAttribute("class", "status-pill tone-emerald");
  }

  const ring = buildCircleRing(remPct, tone, fmtPct(remPct, 0), "剩余可用");
  const heroRight = el(
    "div",
    "",
    el("span", "personal-amount-label", `${selected.label} · ${selected.group_name || "全部分组"}`),
    el("div", "hero-number", fmtCents(remCents)),
    el("div", "hero-subline num", `7 天累计已用 ${fmtCents(usedCents)} / 上限 ${fmtCents(limitCents)}（已用 ${fmtPct(usedPct, 1)}）`),
    buildLinearMeter(remPct, tone)
  );

  let releaseVal = "满额可用";
  let releaseSub = "近 7 天暂无待释放账单";
  if (usedCents > 0 && selected.next_release_at) {
    const relAmt = selected.next_release_cents > 0 ? `+${fmtCents(selected.next_release_cents)}` : "即将释放";
    releaseVal = `${relAmt} · ${fmtCountdown(selected.next_release_at, nowSec)}`;
    releaseSub = `${fmtTime(selected.next_release_at)} 自动滑出`;
  } else if (usedCents > 0) {
    releaseVal = "滑动计算中";
    releaseSub = "等待下次账单快照刷新";
  }

  const todayUsage = selected.usage?.today || { requests: 0, total_tokens: 0, estimated_usd: 0 };
  const errs = personalErrorCount(selected);

  const errTile = el(
    "button",
    "stat-tile clickable",
    el("span", "stat-tile-label", "个人非取消报错 →"),
    el("span", `stat-tile-val num ${errs.todayCount > 0 ? "tone-rose" : ""}`, `今日 ${errs.todayCount} 次`),
    el("span", "stat-tile-sub", `近期流水共 ${errs.nonCancelWeek} 条异常`)
  );
  errTile.type = "button";
  errTile.addEventListener("click", () => {
    onlySelectedMemberFailures = true;
    selectedErrorDate = latestHealthDate() || "";
    setView("errors");
  });

  const statsGrid = el(
    "div",
    "personal-stats-grid",
    el("div", "stat-tile", el("span", "stat-tile-label", "下一次额度释放"), el("span", "stat-tile-val num", releaseVal), el("span", "stat-tile-sub", releaseSub)),
    el("div", "stat-tile", el("span", "stat-tile-label", "今日个人调用"), el("span", "stat-tile-val num", `${fmtInt(todayUsage.requests)} 次 · ${fmtUsd(todayUsage.estimated_usd)}`), el("span", "stat-tile-sub", `${fmtTokens(todayUsage.total_tokens)} tokens · ${fmtRelativeAge(selected.last_used_at, nowSec)}`)),
    errTile
  );

  const cta = el("button", "btn subtle small", "在终端 / Codex / Claude 查此 Key 余量 →");
  cta.id = "personal-terminal-cta";
  cta.type = "button";
  cta.addEventListener("click", openTerminalModal);

  body.replaceChildren(
    el("div", "personal-hero-row", ring, heroRight),
    statsGrid,
    el("div", "personal-cta-row", cta)
  );
}

/* ---------- Render: Pillar 2 (Shared GPT Pro 20X Pool) ---------- */

function accountRemainingDisplay(acct, nowSec) {
  const rem = acct.primary?.remaining_percent;
  const resetAt = acct.primary?.resets_at;
  const staleSnap = acct.captured_at && nowSec - acct.captured_at > 900;
  const resetExpired = resetAt && resetAt <= nowSec;
  if (rem === null || rem === undefined) {
    return { pctText: "待采集", pctNum: 0, tone: "muted", resetText: "等待网关同步最新上游窗口" };
  }
  const tone = acct.status !== "enabled" ? "muted" : quotaTone(rem);
  const resetText = resetAt
    ? resetExpired
      ? `窗口已到点 (${fmtTime(resetAt)}) · 待下次请求刷新`
      : `${fmtCountdown(resetAt, nowSec)}后重置 (${fmtTime(resetAt)})`
    : staleSnap
      ? `上次采样 ${fmtTime(acct.captured_at)}`
      : "周窗口计时中";
  return { pctText: fmtPct(rem, 1), pctNum: rem, tone, resetText };
}

function renderPoolCabin() {
  const body = byId("pool-cabin-body");
  const groupTag = byId("pool-group-tag");
  const healthPill = byId("pool-health-pill");
  const accounts = snapshot && Array.isArray(snapshot.accounts) ? snapshot.accounts : [];
  const nowSec = Math.floor(Date.now() / 1000);
  const liveAccounts = (liveState && liveState.accounts) || {};

  const enabledCount = accounts.filter((a) => a.status === "enabled").length;
  const firstGroup = accounts.find((a) => a.group_name)?.group_name || "共享分组";
  groupTag.textContent = firstGroup;
  healthPill.textContent = `${enabledCount} / ${accounts.length} 在线`;
  healthPill.setAttribute(
    "class",
    enabledCount === accounts.length && accounts.length > 0 ? "status-pill tone-emerald" : "status-pill tone-amber"
  );

  if (!accounts.length) {
    body.replaceChildren(el("div", "empty-state", el("strong", "", "暂无共享 GPT 账号记录")));
    return;
  }

  const validPcts = accounts.map((a) => a.primary?.remaining_percent).filter((v) => typeof v === "number");
  const avgRem = validPcts.length ? validPcts.reduce((s, v) => s + v, 0) / validPcts.length : null;
  const maxConc = accounts.length * 5;
  const totalInflight = liveState && liveState.available ? liveState.total : null;

  const summaryBar = el(
    "div",
    "pool-summary-bar",
    el("span", "", "号池平均剩余 ", el("strong", "num", avgRem === null ? "—" : fmtPct(avgRem, 1))),
    el("span", "", "当前实时总并发 ", el("strong", "num", totalInflight === null ? `— / ${maxConc}` : `${totalInflight} / ${maxConc} 路`)),
    el("span", "", "单号并发上限 ", el("strong", "num", "5 路"))
  );

  const list = el("div", "pool-account-list");
  accounts.forEach((acct, idx) => {
    const win = accountRemainingDisplay(acct, nowSec);
    const inflight = liveState && liveState.available ? (liveAccounts[acct.id] || 0) : 0;
    const statusMeta = accountStatusMeta(acct.status);
    const reqInfo = acct.today_requests > 0
      ? `今日 ${fmtInt(acct.today_requests)} 次${acct.today_success_rate != null ? ` (${fmtPct(acct.today_success_rate, 1)})` : ""}`
      : "今日暂无调度";

    list.append(
      el(
        "div",
        "pool-account-item",
        el(
          "div",
          "pool-account-top",
          el(
            "div",
            "pool-account-id",
            el("span", "pool-account-name", `${acct.label}`),
            el("span", "badge tone-sky", (acct.plan || "pro").toUpperCase()),
            el("span", `badge tone-${statusMeta.tone}`, statusMeta.label)
          ),
          el("span", `pool-account-pct num tone-${win.tone}`, win.pctText)
        ),
        buildLinearMeter(win.pctNum, win.tone),
        el(
          "div",
          "pool-account-meta",
          el(
            "span",
            "slot-group",
            el("span", "", "并发槽位"),
            buildSlotPips(inflight, 5),
            el("span", "num", liveState && liveState.available ? `${inflight}/5` : `—/5`)
          ),
          el("span", "num", `7d 参考 ${fmtCents(acct.weekly_used_cents)}`),
          el("span", "", reqInfo),
          el("span", "num", win.resetText)
        )
      )
    );
  });

  body.replaceChildren(summaryBar, list);
}

/* ---------- Render: Pulse Strip, 14D Chart, Model Weather & Digest ---------- */

function latestHealthDate() {
  const days = snapshot?.request_health?.days || [];
  return days.length ? days[days.length - 1].date : "";
}

function renderPulseStrip() {
  const strip = byId("pulse-strip");
  const todayH = snapshot?.request_health?.today;
  const todayUsage = snapshot?.totals?.today;
  const weekUsage = snapshot?.totals?.week;
  const speed = snapshot?.speed;
  const activeTodayKeys = currentKeys().filter((k) => (k.usage?.today?.requests || 0) > 0).length;

  const netRate = todayH ? fmtPct(todayH.effective_success_rate ?? todayH.success_rate, 2) : "—";
  const rawRate = todayH ? fmtPct(todayH.success_rate, 2) : "—";
  const sysFails = todayH?.system_failed_count ?? todayH?.failed ?? 0;

  const cacheHit = todayUsage && (todayUsage.input_tokens + todayUsage.cached_tokens) > 0
    ? (todayUsage.cached_tokens * 100) / todayUsage.input_tokens
    : null;

  const cards = [
    el(
      "div",
      "kpi-card",
      el("span", "kpi-label", "今日系统净成功率"),
      el("span", `kpi-value num ${sysFails > 0 ? "tone-amber" : "tone-emerald"}`, netRate),
      el("span", "kpi-sub num", `已剔除 499 取消 ${fmtInt(todayH?.client_cancelled_count || 0)} 次 · 原始 ${rawRate}`)
    ),
    el(
      "div",
      "kpi-card",
      el("span", "kpi-label", "今日请求与系统异常"),
      el("span", "kpi-value num", `${fmtInt(todayH?.requests ?? todayUsage?.requests)} 次`),
      el("span", "kpi-sub num", `今日活跃 ${activeTodayKeys} 人 · 异常 ${fmtInt(sysFails)} 次`)
    ),
    el(
      "div",
      "kpi-card",
      el("span", "kpi-label", "解码速度 P50 / 首字延迟"),
      el("span", "kpi-value num", speed?.decode_p50 != null ? `${speed.decode_p50} tok/s` : "—"),
      el("span", "kpi-sub num", `P90 ${speed?.decode_p90 ?? "—"} tok/s · 平均首字 ${fmtDuration(todayH?.average_first_response_ms)}`)
    ),
    el(
      "div",
      "kpi-card",
      el("span", "kpi-label", "今日团队 Token 吞吐"),
      el("span", "kpi-value num", fmtTokens(todayUsage?.total_tokens)),
      el("span", "kpi-sub num", `缓存读占比 ${cacheHit === null ? "—" : fmtPct(clamp(cacheHit, 0, 100), 1)}`)
    ),
    el(
      "div",
      "kpi-card",
      el("span", "kpi-label", "今日 / 近 7 天参考消耗"),
      el("span", "kpi-value num", fmtUsd(todayUsage?.estimated_usd)),
      el("span", "kpi-sub num", `近 7 天累计 ${fmtUsd(weekUsage?.estimated_usd)}`)
    ),
  ];
  strip.replaceChildren(...cards);
}

function renderTrendChart() {
  const wrap = byId("overview-trend-chart");
  const caption = byId("overview-trend-caption");
  const days = snapshot?.request_health?.days || [];
  if (!days.length) {
    wrap.replaceChildren(el("div", "empty-state", el("strong", "", "暂无 14 天请求健康趋势数据")));
    return;
  }

  const width = 700;
  const height = 220;
  const padL = 42;
  const padR = 38;
  const padT = 18;
  const padB = 34;
  const plotW = width - padL - padR;
  const plotH = height - padT - padB;
  const maxReq = Math.max(10, ...days.map((d) => d.requests || 0));

  const svg = svgEl("svg", { class: "trend-svg", viewBox: `0 0 ${width} ${height}` });

  for (const frac of [0, 0.5, 1]) {
    const y = padT + plotH * (1 - frac);
    svg.append(svgEl("line", { class: "chart-grid-line", x1: padL, y1: y, x2: width - padR, y2: y }));
    svg.append(
      svgEl("text", { class: "chart-axis-text", x: padL - 6, y: y + 3.5, "text-anchor": "end" }, fmtTokens(Math.round(maxReq * frac)))
    );
  }

  const stepW = plotW / days.length;
  const barW = Math.min(26, Math.max(10, stepW * 0.56));
  const ratePoints = [];

  days.forEach((day, idx) => {
    const xCenter = padL + stepW * (idx + 0.5);
    const barX = xCenter - barW / 2;
    const req = day.requests || 0;
    const cancel = day.client_cancelled_count || 0;
    const sysFail = day.system_failed_count ?? Math.max(0, (day.failed || 0) - cancel);
    const ok = Math.max(0, req - cancel - sysFail);

    const hOk = (ok / maxReq) * plotH;
    const hCancel = (cancel / maxReq) * plotH;
    const hSys = (sysFail / maxReq) * plotH;

    const group = svgEl("g", { class: `chart-day-group${day.date === selectedErrorDate ? " is-selected" : ""}` });
    group.append(svgEl("rect", { class: "chart-day-hit", x: padL + stepW * idx, y: padT, width: stepW, height: plotH + 24, rx: 4 }));

    let yCursor = padT + plotH;
    if (hOk > 0.5) {
      yCursor -= hOk;
      group.append(svgEl("rect", { class: "chart-bar-ok", x: barX, y: yCursor, width: barW, height: hOk, rx: 2 }));
    }
    if (hCancel > 0.5) {
      yCursor -= hCancel;
      group.append(svgEl("rect", { class: "chart-bar-cancel", x: barX, y: yCursor, width: barW, height: hCancel }));
    }
    if (hSys > 0.5) {
      const visH = Math.max(2.5, hSys);
      yCursor -= visH;
      group.append(svgEl("rect", { class: "chart-bar-sys", x: barX, y: yCursor, width: barW, height: visH, rx: 1.5 }));
    }

    const eff = day.requests ? (day.effective_success_rate ?? day.success_rate) : 100;
    const lineY = padT + plotH * (1 - clamp(eff, 0, 100) / 100);
    ratePoints.push({ x: xCenter, y: lineY, day, eff, sysFail, cancel });

    if (idx % 2 === 0 || idx === days.length - 1) {
      group.append(
        svgEl("text", { class: "chart-axis-text", x: xCenter, y: height - 10, "text-anchor": "middle" }, day.date.slice(5))
      );
    }

    const updateCaption = () => {
      caption.textContent = `${day.date}：总请求 ${fmtInt(req)} · 系统净成功率 ${fmtPct(eff, 2)} · 网关/上游/配置异常 ${fmtInt(sysFail)} · 主动取消 ${fmtInt(cancel)}（点击查看诊断）`;
    };
    group.addEventListener("mouseenter", updateCaption);
    group.addEventListener("click", () => {
      selectedErrorDate = day.date;
      selectedLayerFilter = "all";
      setView("errors");
    });
    svg.append(group);
  });

  if (ratePoints.length > 1) {
    svg.append(
      svgEl("polyline", {
        class: "chart-rate-line",
        points: ratePoints.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(" "),
      })
    );
    for (const pt of ratePoints) {
      svg.append(svgEl("circle", { class: "chart-rate-dot", cx: pt.x.toFixed(1), cy: pt.y.toFixed(1), r: 3 }));
    }
  }

  wrap.replaceChildren(svg);
}

function renderModelWeatherBoard() {
  const list = byId("overview-model-weather");
  const weather = snapshot?.model_weather || [];
  if (!weather.length) {
    const fallback = snapshot?.speed?.models || [];
    if (!fallback.length) {
      list.replaceChildren(el("div", "empty-state", el("strong", "", "暂无模型实时气象数据")));
      return;
    }
  }

  const activeItems = weather.filter((w) => w.week_requests > 0 || w.today_requests > 0).slice(0, 6);
  const items = activeItems.length ? activeItems : weather.slice(0, 6);

  const rows = items.map((item) => {
    const meta = WEATHER_META[item.status] || WEATHER_META.idle;
    const rateStr = item.today_requests > 0
      ? `今日 ${fmtPct(item.today_success_rate, 1)}`
      : `7日 ${fmtPct(item.week_effective_success_rate, 1)}`;
    const subParts = [
      `今日 ${fmtInt(item.today_requests)} / 7日 ${fmtInt(item.week_requests)} 次`,
      item.today_overload_count > 0 ? `过载 ${item.today_overload_count} 次` : "今日 0 官方过载",
    ];
    return el(
      "div",
      "weather-row",
      el("div", "", el("div", "weather-model-name mono", item.model), el("div", "weather-model-sub", subParts.join(" · "))),
      el("div", "weather-metric", el("strong", "num", item.decode_p50 != null ? `${item.decode_p50} tok/s` : "—"), el("span", "", `首字 ${fmtDuration(item.ttft_p50_ms)}`)),
      el("div", "weather-metric", el("strong", "num", rateStr), el("span", "", "净成功率")),
      el("span", `badge tone-${meta.tone}`, meta.label)
    );
  });

  list.replaceChildren(...rows);
}

/* ---------- Shared Signature Card Renderer (Pillar 3) ---------- */

function layerBadge(layerKey) {
  const meta = LAYER_META[layerKey] || LAYER_META.upstream_openai;
  return el("span", `badge tone-${meta.tone}`, meta.label);
}

function buildSignatureCard(sig, showCounts = true) {
  const sevClass = `sev-${sig.severity || "warn"}`;
  const codes = (sig.status_codes || sig.codes || []).map((c) => `HTTP ${c}`).join(" / ") || "无固定状态码";
  const topLeft = el(
    "div",
    "",
    el("div", "sig-title", sig.title),
    el(
      "div",
      "sig-badges",
      layerBadge(sig.layer),
      el("span", "badge tone-muted mono", codes)
    )
  );
  const topRight = showCounts ? el("span", "sig-count", `${fmtInt(sig.count)} 次`) : null;

  const explainBlock = el(
    "div",
    "sig-explain-block",
    el("div", "", el("span", "sig-row-label", "报错含义"), sig.meaning),
    el("div", "", el("span", "sig-row-label action", "建议与迭代"), sig.action)
  );

  const footerItems = [];
  if (showCounts) {
    const models = (sig.top_models || []).length ? `模型: ${sig.top_models.join(", ")}` : "多为入口校验/路由前拦截";
    footerItems.push(el("span", "", models));
    const metaRight = [];
    if (sig.affected_keys > 0) metaRight.push(`波及 ${sig.affected_keys} 位成员`);
    if (sig.last_seen_at) metaRight.push(`最近 ${fmtTime(sig.last_seen_at)}`);
    footerItems.push(el("span", "num", metaRight.join(" · ") || "—"));
  }

  const card = el(
    "div",
    `signature-card ${sevClass}`,
    el("div", "sig-top", topLeft, topRight),
    explainBlock
  );
  if (sig.sample_message) {
    card.append(el("div", "cell-sub mono", sig.sample_message));
  }
  if (footerItems.length) {
    card.append(el("div", "sig-meta-footer", ...footerItems));
  }
  return card;
}

function renderOverviewErrorDigest() {
  const tabsWrap = byId("overview-error-day-tabs");
  const layerRow = byId("overview-error-layers");
  const digestWrap = byId("overview-error-digest");
  const health = snapshot?.request_health;
  const days = health?.days || [];
  const todayH = health?.today;
  const yesterdayH = days.length >= 2 ? days[days.length - 2] : null;
  const weekH = health?.week;

  const tabDefs = [
    { id: "today", label: `今天 (${todayH ? fmtInt(todayH.system_failed_count ?? todayH.failed) : 0})` },
    { id: "yesterday", label: `昨天 (${yesterdayH ? fmtInt(yesterdayH.system_failed_count ?? yesterdayH.failed) : 0})` },
    { id: "week", label: `近 7 天 (${weekH ? fmtInt(weekH.system_failed_count ?? weekH.failed) : 0})` },
  ];
  tabsWrap.replaceChildren(
    ...tabDefs.map((t) => {
      const btn = el("button", `seg-btn${overviewErrorTab === t.id ? " is-active" : ""}`, t.label);
      btn.type = "button";
      btn.addEventListener("click", () => {
        overviewErrorTab = t.id;
        renderOverviewErrorDigest();
      });
      return btn;
    })
  );

  const summary = overviewErrorTab === "yesterday" ? (yesterdayH || todayH) : overviewErrorTab === "week" ? weekH : todayH;
  if (!summary) {
    layerRow.replaceChildren();
    digestWrap.replaceChildren(el("div", "empty-state", el("strong", "", "暂无错误摘要数据")));
    return;
  }

  const layerMap = Object.fromEntries((summary.error_layers || []).map((x) => [x.layer, x.count]));
  layerRow.replaceChildren(
    ...Object.entries(LAYER_META).map(([key, meta]) =>
      el("div", "layer-mini-chip", el("span", `badge tone-${meta.tone}`, meta.label), el("strong", "num", fmtInt(layerMap[key] || 0)))
    )
  );

  const sigs = (summary.signatures || []).slice(0, 6);
  if (!sigs.length) {
    digestWrap.replaceChildren(
      el(
        "div",
        "empty-state",
        el("strong", "", "该时段中转站与共享池运行零异常"),
        el("p", "", `累计处理 ${fmtInt(summary.requests)} 次请求，可切换「昨天」或「近 7 天」回顾历史诊断记录。`)
      )
    );
    return;
  }
  digestWrap.replaceChildren(...sigs.map((s) => buildSignatureCard(s, true)));
}

/* ---------- Render: View 2 (#errors) Full Daily Error Observatory ---------- */

function findSelectedErrorSummary() {
  const health = snapshot?.request_health;
  if (!health) return { title: "未知周期", summary: null, targetDate: "" };
  const days = health.days || [];
  if (selectedErrorDate === "week") {
    return { title: "近 7 天异常全量汇总", summary: health.week, targetDate: "week" };
  }
  const match = days.find((d) => d.date === selectedErrorDate) || days[days.length - 1];
  const dateStr = match ? match.date : "";
  const todayDate = days.length ? days[days.length - 1].date : "";
  const yestDate = days.length >= 2 ? days[days.length - 2].date : "";
  const tag = dateStr === todayDate ? "今天" : dateStr === yestDate ? "昨天" : dateStr;
  return { title: `${dateStr}（${tag}）`, summary: match || health.today, targetDate: dateStr };
}

function renderErrorsView() {
  const health = snapshot?.request_health;
  const days = (health?.days || []).slice().reverse();
  const dayStrip = byId("errors-day-strip");
  const weekBtn = byId("errors-scope-week-btn");
  const { title, summary, targetDate } = findSelectedErrorSummary();

  weekBtn.setAttribute("class", targetDate === "week" ? "btn subtle is-active" : "btn subtle");
  byId("errors-selected-summary").textContent = summary
    ? `${title} · 请求 ${fmtInt(summary.requests)} · 净成功率 ${fmtPct(summary.effective_success_rate ?? summary.success_rate, 2)}`
    : title;

  const todayDate = latestHealthDate();
  const yestDate = days[1]?.date || "";
  dayStrip.replaceChildren(
    ...days.map((d) => {
      const sysFail = d.system_failed_count ?? Math.max(0, (d.failed || 0) - (d.client_cancelled_count || 0));
      const label = d.date === todayDate ? `今天 ${d.date.slice(5)}` : d.date === yestDate ? `昨天 ${d.date.slice(5)}` : d.date.slice(5);
      const pill = el(
        "button",
        `day-pill${targetDate === d.date ? " is-selected" : ""}`,
        el("div", "day-pill-top", el("span", "", label), el("span", `badge tone-${sysFail > 0 ? "rose" : "emerald"} num`, `${sysFail}`)),
        el("div", "day-pill-sub num", `${fmtInt(d.requests)} 次 · ${fmtPct(d.effective_success_rate ?? d.success_rate, 1)}`)
      );
      pill.type = "button";
      pill.addEventListener("click", () => {
        selectedErrorDate = d.date;
        renderErrorsView();
      });
      return pill;
    })
  );

  const layerCardsWrap = byId("errors-layer-cards");
  const layerCounts = Object.fromEntries((summary?.error_layers || []).map((l) => [l.layer, l.count]));
  layerCardsWrap.replaceChildren(
    ...Object.entries(LAYER_META).map(([key, meta]) => {
      const btn = el(
        "button",
        `layer-card${selectedLayerFilter === key ? " is-active" : ""}`,
        el("div", "layer-card-head", el("span", `badge tone-${meta.tone}`, meta.label), el("span", "badge tone-muted", meta.owner)),
        el("div", "layer-card-count num", `${fmtInt(layerCounts[key] || 0)} 次`),
        el("div", "layer-card-desc", meta.desc)
      );
      btn.type = "button";
      btn.addEventListener("click", () => {
        selectedLayerFilter = selectedLayerFilter === key ? "all" : key;
        byId("failure-layer-filter").value = selectedLayerFilter;
        renderErrorsView();
      });
      return btn;
    })
  );

  byId("errors-sig-heading").textContent = `${title} · 报错拆解与白话含义`;
  byId("errors-sig-filter-label").textContent =
    selectedLayerFilter === "all" ? "显示全部 4 个层级" : `仅显示：${LAYER_META[selectedLayerFilter]?.label || selectedLayerFilter}（再点一次恢复全部）`;

  const sigList = byId("errors-signature-list");
  const sigs = (summary?.signatures || []).filter((s) => selectedLayerFilter === "all" || s.layer === selectedLayerFilter);
  if (!sigs.length) {
    sigList.replaceChildren(
      el("div", "empty-state", el("strong", "", "该日期 / 层级下无报错记录"), el("p", "", "可点击上方其他日期或清除层级筛选查看。"))
    );
  } else {
    sigList.replaceChildren(...sigs.map((s) => buildSignatureCard(s, true)));
  }

  renderFailureTable(targetDate);
  byId("error-dictionary-grid").replaceChildren(...ERROR_DICTIONARY_REF.map((entry) => buildSignatureCard(entry, false)));
}

function renderFailureTable(targetDate) {
  const tbody = byId("failure-table-body");
  const note = byId("failure-feed-note");
  const myOnlyBtn = byId("failure-my-only-btn");
  const search = (byId("failure-search").value || "").trim().toLowerCase();
  const layerFilter = byId("failure-layer-filter").value || "all";
  const selectedMember = findSelectedKey();

  myOnlyBtn.disabled = !selectedMember;
  myOnlyBtn.textContent = selectedMember ? `只看 ${selectedMember.label}` : "只看当前所选成员（先在顶部选择）";
  myOnlyBtn.setAttribute("class", onlySelectedMemberFailures && selectedMember ? "btn subtle is-active" : "btn subtle");
  myOnlyBtn.setAttribute("aria-pressed", String(Boolean(onlySelectedMemberFailures && selectedMember)));

  const failures = snapshot?.recent_failures || [];
  const filtered = failures.filter((item) => {
    if (targetDate && targetDate !== "week" && item.date && item.date !== targetDate) {
      // Allow seeing all recent logs if current day is selected cleanly; wait - date match improves clarity!
      // Wait: if selected day is older than the latest 200 logs, show hint below!
      return false;
    }
    if (layerFilter !== "all" && item.layer !== layerFilter) return false;
    if (onlySelectedMemberFailures && selectedMember) {
      if (item.key_id !== selectedMember.id && item.key_label !== selectedMember.label) return false;
    }
    if (search) {
      const hay = [item.key_label, item.model, item.account_label, item.title, item.meaning, item.message, String(item.status_code)]
        .filter(Boolean)
        .join(" ")
        .toLowerCase();
      if (!hay.includes(search)) return false;
    }
    return true;
  });

  if (!filtered.length) {
    const oldestDate = failures.length ? failures[failures.length - 1].date : "";
    const msg = targetDate && targetDate !== "week" && oldestDate && targetDate < oldestDate
      ? `该日期（${targetDate}）的逐条流水已被最新 200 条覆盖（当前流水保留自 ${oldestDate} 起），请查看上方当日聚合拆解或切换「近 7 天汇总」。`
      : "当前筛选条件下没有对应的异常流水（最多保留近 14 天最新 200 条非 2xx 请求）。";
    const td = el("td", "muted", msg);
    td.setAttribute("colspan", "8");
    tbody.replaceChildren(el("tr", "", td));
  } else {
    tbody.replaceChildren(
      ...filtered.slice(0, 120).map((f) => {
        const meta = LAYER_META[f.layer] || LAYER_META.upstream_openai;
        return el(
          "tr",
          "",
          el("td", "num", fmtTime(f.occurred_at)),
          el("td", "", el("div", "cell-primary", f.title || f.category), el("span", `badge tone-${meta.tone}`, meta.label)),
          el("td", "mono num", `HTTP ${f.status_code}`),
          el("td", "", f.key_label || "未携带/匿名"),
          el("td", "mono", f.model || "—"),
          el("td", "", f.account_label || "未分配前"),
          el("td", "num", fmtDuration(f.duration_ms)),
          el("td", "", el("div", "cell-primary", f.message || "—"), el("div", "cell-sub", f.meaning || ""))
        );
      })
    );
  }
  note.textContent = `已保留近 14 天最近 ${fmtInt(failures.length)} 条原始脱敏异常流水，当前匹配 ${fmtInt(filtered.length)} 条。`;
}

/* ---------- Render: View 3 (#keys) Members & Quotas ---------- */

function renderKeysView() {
  const keys = currentKeys();
  const search = (byId("key-search").value || "").trim().toLowerCase();
  const statusFilter = byId("key-status-filter").value || "all";
  const sortMode = byId("key-sort").value || "used_desc";

  const filtered = keys.filter((k) => {
    if (statusFilter !== "all" && k.status !== statusFilter) return false;
    if (search) {
      const hay = `${k.label} ${k.group_name || ""}`.toLowerCase();
      if (!hay.includes(search)) return false;
    }
    return true;
  });

  filtered.sort((a, b) => {
    if (sortMode === "remaining_asc") return (a.weekly_remaining_cents ?? 24000) - (b.weekly_remaining_cents ?? 24000);
    if (sortMode === "today_desc") return (b.usage?.today?.requests || 0) - (a.usage?.today?.requests || 0);
    if (sortMode === "recent_desc") return (b.last_used_at || 0) - (a.last_used_at || 0);
    return (b.weekly_used_cents || 0) - (a.weekly_used_cents || 0) || (b.usage?.week?.requests || 0) - (a.usage?.week?.requests || 0);
  });

  const enabled = keys.filter((k) => k.status === "enabled").length;
  const nearLimit = keys.filter((k) => (k.weekly_remaining_cents ?? 24000) <= 2400).length;
  const total7dCents = keys.reduce((s, k) => s + (k.weekly_used_cents || 0), 0);
  byId("keys-summary-strip").replaceChildren(
    el("div", "kpi-card", el("span", "kpi-label", "启用成员 Key"), el("span", "kpi-value num", `${enabled} / ${keys.length}`), el("span", "kpi-sub", "每人独立专属 Key")),
    el("div", "kpi-card", el("span", "kpi-label", "接近或到达 $240 上限"), el("span", `kpi-value num ${nearLimit > 0 ? "tone-amber" : "tone-emerald"}`, `${nearLimit} 人`), el("span", "kpi-sub", "余量 <= $24.00 预警")),
    el("div", "kpi-card", el("span", "kpi-label", "当前成员 7 天已计费合计"), el("span", "kpi-value num", fmtCents(total7dCents)), el("span", "kpi-sub", "按滚动 168h 滑窗统计"))
  );

  const nowSec = Math.floor(Date.now() / 1000);
  const tbody = byId("keys-table-body");
  if (!filtered.length) {
    const emptyTd = el("td", "muted", "没有匹配的成员或 Key。");
    emptyTd.setAttribute("colspan", "7");
    tbody.replaceChildren(el("tr", "", emptyTd));
    return;
  }

  tbody.replaceChildren(
    ...filtered.map((k) => {
      const limitCents = k.weekly_limit_cents || 24000;
      const usedCents = clamp(k.weekly_used_cents || 0, 0, limitCents);
      const remCents = k.weekly_remaining_cents ?? Math.max(0, limitCents - usedCents);
      const remPct = clamp((remCents / limitCents) * 100, 0, 100);
      const tone = k.status === "disabled" ? "muted" : remCents === 0 ? "rose" : remPct <= 25 ? "amber" : "emerald";
      const relStr = usedCents > 0 && k.next_release_at
        ? `+${fmtCents(k.next_release_cents)} · ${fmtCountdown(k.next_release_at, nowSec)}`
        : "满额 · 无待释放";
      const isFocused = k.id === selectedKeyId;
      const pinBtn = el("button", `btn small ${isFocused ? "primary" : "subtle"}`, isFocused ? "当前视角" : "设为视角");
      pinBtn.type = "button";
      pinBtn.addEventListener("click", (ev) => {
        ev.stopPropagation();
        selectMember(isFocused ? "" : k.id);
      });

      const row = el(
        "tr",
        `is-clickable${isFocused ? " is-focused" : ""}`,
        el("td", "", el("div", "cell-primary", k.label), el("div", "cell-sub mono", k.display_id || k.id)),
        el("td", "", el("span", `badge tone-${k.status === "enabled" ? "emerald" : "muted"}`, k.status === "enabled" ? "已启用" : "已停用"), el("div", "cell-sub", k.group_name || "全部分组（未限制）")),
        el(
          "td",
          "",
          el(
            "div",
            "meter-stack",
            el("div", "meter-stack-top", el("strong", "num", `余 ${fmtCents(remCents)}`), el("span", "num muted", `已用 ${fmtCents(usedCents)}`)),
            buildLinearMeter(remPct, tone)
          )
        ),
        el("td", "num", el("div", "", relStr), el("div", "cell-sub", k.next_release_at && usedCents > 0 ? fmtTime(k.next_release_at) : "—")),
        el("td", "num", fmtInt(k.usage?.today?.requests || 0), el("div", "cell-sub", fmtUsd(k.usage?.today?.estimated_usd || 0))),
        el("td", "num", fmtTokens(k.usage?.week?.total_tokens || 0), el("div", "cell-sub", fmtUsd(k.usage?.week?.estimated_usd || 0))),
        el("td", "", pinBtn)
      );
      row.addEventListener("click", () => openMemberDrawer(k));
      return row;
    })
  );
}

function openMemberDrawer(keyItem) {
  const nowSec = Math.floor(Date.now() / 1000);
  const limitCents = keyItem.weekly_limit_cents || 24000;
  const usedCents = clamp(keyItem.weekly_used_cents || 0, 0, limitCents);
  const remCents = keyItem.weekly_remaining_cents ?? Math.max(0, limitCents - usedCents);
  const periods = [
    { name: "今日", data: keyItem.usage?.today },
    { name: "近 7 天", data: keyItem.usage?.week },
    { name: "累计归档", data: keyItem.usage?.recorded },
  ];

  byId("drawer-eyebrow").textContent = "MEMBER QUOTA & TELEMETRY";
  byId("drawer-title").textContent = keyItem.label;
  byId("drawer-body").replaceChildren(
    el(
      "div",
      "drawer-kv-grid",
      el("div", "acct-fact", el("span", "", "$240 剩余额度"), el("strong", "num", fmtCents(remCents))),
      el("div", "acct-fact", el("span", "", "7 天已用"), el("strong", "num", fmtCents(usedCents))),
      el("div", "acct-fact", el("span", "", "所属分组"), el("strong", "", keyItem.group_name || "全部分组（未限制）")),
      el("div", "acct-fact", el("span", "", "最近活跃"), el("strong", "", fmtRelativeAge(keyItem.last_used_at, nowSec))),
      el("div", "acct-fact", el("span", "", "协议 / 策略"), el("strong", "mono", `${keyItem.protocol || "openai_compat"}`)),
      el("div", "acct-fact", el("span", "", "下一次释放"), el("strong", "num", usedCents > 0 && keyItem.next_release_at ? `+${fmtCents(keyItem.next_release_cents)} · ${fmtTime(keyItem.next_release_at)}` : "暂无"))
    ),
    ...periods.map((p) =>
      el(
        "div",
        "acct-detail-card",
        el("strong", "", `${p.name}用量明细`),
        el(
          "div",
          "acct-facts",
          el("div", "acct-fact", el("span", "", "请求次数"), el("strong", "num", fmtInt(p.data?.requests || 0))),
          el("div", "acct-fact", el("span", "", "总 Token"), el("strong", "num", fmtTokens(p.data?.total_tokens || 0))),
          el("div", "acct-fact", el("span", "", "输入 / 缓存读"), el("strong", "num", `${fmtTokens(p.data?.input_tokens || 0)} / ${fmtTokens(p.data?.cached_tokens || 0)}`)),
          el("div", "acct-fact", el("span", "", "参考消耗"), el("strong", "num", fmtUsd(p.data?.estimated_usd || 0)))
        )
      )
    )
  );
  byId("drawer-root").hidden = false;
}

/* ---------- Render: View 4 (#accounts) Shared Accounts ---------- */

function renderAccountsView() {
  const accounts = snapshot?.accounts || [];
  const claude = snapshot?.claude_accounts || [];
  const liveAccounts = (liveState && liveState.accounts) || {};
  const nowSec = Math.floor(Date.now() / 1000);
  byId("accounts-updated").textContent = snapshot?.generated_at ? `快照生成于 ${fmtTime(snapshot.generated_at)}` : "—";

  const enabled = accounts.filter((a) => a.status === "enabled").length;
  const totalWeekCents = accounts.reduce((s, a) => s + (a.weekly_used_cents || 0), 0);
  const totalTodayReq = accounts.reduce((s, a) => s + (a.today_requests || 0), 0);
  byId("accounts-pool-summary").replaceChildren(
    el("div", "kpi-card", el("span", "kpi-label", "共享 GPT 账号数"), el("span", "kpi-value num", `${enabled} / ${accounts.length}`), el("span", "kpi-sub", "均分担团队请求轮询")),
    el("div", "kpi-card", el("span", "kpi-label", "实时活跃并发"), el("span", "kpi-value num", liveState?.available ? `${liveState.total} / ${accounts.length * 5}` : "—"), el("span", "kpi-sub", "单账号硬并发上限 5 路")),
    el("div", "kpi-card", el("span", "kpi-label", "近 7 天号池参考总耗"), el("span", "kpi-value num", fmtCents(totalWeekCents)), el("span", "kpi-sub", `今日已路由 ${fmtInt(totalTodayReq)} 次请求`))
  );

  byId("accounts-card-grid").replaceChildren(
    ...accounts.map((acct) => {
      const win = accountRemainingDisplay(acct, nowSec);
      const statusMeta = accountStatusMeta(acct.status);
      const inflight = liveState?.available ? (liveAccounts[acct.id] || 0) : 0;
      const tech = el("details", "tech-details", el("summary", "", "查看管理员只读技术元数据"));
      tech.append(
        el("div", "cell-sub", `公开 ID: ${acct.id} · 代理: ${acct.proxy_state} · 容量覆盖: ${acct.capacity_override ? "是" : "否"}`),
        acct.email ? el("div", "cell-sub", `账号标识: ${acct.email}`) : null,
        acct.reset_available_count != null ? el("div", "cell-sub num", `可用重置次数: ${acct.reset_available_count}`) : null,
        acct.spark_primary?.remaining_percent != null ? el("div", "cell-sub num", `Spark 窗口余量: ${fmtPct(acct.spark_primary.remaining_percent, 1)}`) : null,
        acct.subscription_renews_at ? el("div", "cell-sub", `订阅续期参考: ${fmtTime(acct.subscription_renews_at)}`) : null
      );
      return el(
        "div",
        "acct-detail-card",
        el(
          "div",
          "pool-account-top",
          el("div", "pool-account-id", el("span", "pool-account-name", acct.label), el("span", "badge tone-sky", (acct.plan || "pro").toUpperCase()), el("span", `badge tone-${statusMeta.tone}`, statusMeta.label)),
          el("span", `pool-account-pct num tone-${win.tone}`, win.pctText)
        ),
        buildLinearMeter(win.pctNum, win.tone),
        el(
          "div",
          "acct-facts",
          el("div", "acct-fact", el("span", "", "所属分组"), el("strong", "", acct.group_name || "未分组")),
          el("div", "acct-fact", el("span", "", "当前并发"), el("strong", "slot-group num", buildSlotPips(inflight, 5), liveState?.available ? `${inflight}/5` : "—/5")),
          el("div", "acct-fact", el("span", "", "下次窗口重置"), el("strong", "num", win.resetText)),
          el("div", "acct-fact", el("span", "", "7d / 今日调度"), el("strong", "num", `${fmtCents(acct.weekly_used_cents)} · 今日 ${fmtInt(acct.today_requests)} 次`))
        ),
        tech
      );
    })
  );

  const claudeSec = byId("claude-accounts-section");
  claudeSec.hidden = claude.length === 0;
  if (claude.length) {
    byId("claude-accounts-grid").replaceChildren(
      ...claude.map((c) =>
        el(
          "div",
          "acct-detail-card",
          el("div", "pool-account-top", el("strong", "", c.label), el("span", "badge tone-muted", (c.plan || "pro").toUpperCase())),
          el("div", "acct-facts",
            el("div", "acct-fact", el("span", "", "5h 窗口剩余"), el("strong", "num", fmtPct(c.five_hour?.remaining_percent, 1))),
            el("div", "acct-fact", el("span", "", "7d 窗口剩余"), el("strong", "num", fmtPct(c.seven_day?.remaining_percent, 1)))
          )
        )
      )
    );
  }
}

/* ---------- Render: View 5 (#usage) Groups & Models ---------- */

function renderUsageView() {
  for (const btn of document.querySelectorAll("[data-period]")) {
    const match = btn.dataset.period === activePeriod;
    btn.setAttribute("class", match ? "seg-btn is-active" : "seg-btn");
  }

  const total = snapshot?.totals?.[activePeriod] || {};
  const openaiPool = snapshot?.pool_usage?.openai?.[activePeriod] || {};
  const claudePool = snapshot?.pool_usage?.claude?.[activePeriod] || {};
  const unattPool = snapshot?.pool_usage?.unattributed?.[activePeriod] || {};

  byId("usage-pool-cards").replaceChildren(
    el("div", "kpi-card", el("span", "kpi-label", "团队全量合计"), el("span", "kpi-value num", fmtUsd(total.estimated_usd)), el("span", "kpi-sub num", `${fmtInt(total.requests)} 次 · ${fmtTokens(total.total_tokens)} tokens`)),
    el("div", "kpi-card", el("span", "kpi-label", "OpenAI GPT 号池"), el("span", "kpi-value num", fmtUsd(openaiPool.estimated_usd)), el("span", "kpi-sub num", `${fmtInt(openaiPool.requests)} 次 · ${fmtTokens(openaiPool.total_tokens)} tokens`)),
    el("div", "kpi-card", el("span", "kpi-label", "Claude 订阅池"), el("span", "kpi-value num", fmtUsd(claudePool.estimated_usd)), el("span", "kpi-sub num", `${fmtInt(claudePool.requests)} 次 · ${fmtTokens(claudePool.total_tokens)} tokens`)),
    el("div", "kpi-card", el("span", "kpi-label", "历史 / 未归属来源"), el("span", "kpi-value num", fmtUsd(unattPool.estimated_usd)), el("span", "kpi-sub num", `${fmtInt(unattPool.requests)} 次 · ${fmtTokens(unattPool.total_tokens)} tokens`))
  );

  const groups = (snapshot?.key_groups || []).slice().sort((a, b) => {
    const ua = a.usage?.[activePeriod] || {};
    const ub = b.usage?.[activePeriod] || {};
    return (ub.total_tokens || 0) - (ua.total_tokens || 0) || (ub.requests || 0) - (ua.requests || 0) || (b.key_count || 0) - (a.key_count || 0);
  });

  byId("groups-table-body").replaceChildren(
    ...groups.map((g) => {
      const u = g.usage?.[activePeriod] || {};
      return el(
        "tr",
        "",
        el("td", "", el("div", "cell-primary", g.label), el("div", "cell-sub", g.kind === "named" ? "指定分组路由" : g.kind === "unrestricted" ? "未限制分组" : "已归档历史")),
        el("td", "num", fmtInt(g.key_count)),
        el("td", "num", fmtInt(u.requests)),
        el("td", "num", fmtTokens(u.total_tokens)),
        el("td", "num", fmtTokens(u.cached_tokens)),
        el("td", "num", fmtUsd(u.estimated_usd))
      );
    })
  );

  const models = snapshot?.models_week || [];
  byId("usage-models-body").replaceChildren(
    ...models.map((m) => {
      const u = m.usage || {};
      const cachePct = u.input_tokens > 0 ? clamp((u.cached_tokens * 100) / u.input_tokens, 0, 100) : null;
      return el(
        "tr",
        "",
        el("td", "mono cell-primary", m.model),
        el("td", "num", fmtInt(u.requests)),
        el("td", "num", fmtTokens(u.total_tokens)),
        el("td", "num", fmtTokens(u.output_tokens)),
        el("td", "num", cachePct === null ? "—" : fmtPct(cachePct, 1)),
        el("td", "num", fmtUsd(u.estimated_usd))
      );
    })
  );
}

/* ---------- Render: View 6 (#models) Model Catalog & Weather ---------- */

function renderModelsView() {
  const catalog = snapshot?.model_catalog || [];
  const weatherMap = Object.fromEntries((snapshot?.model_weather || []).map((w) => [w.model, w]));
  const search = (byId("model-search").value || "").trim().toLowerCase();
  const statusFilter = byId("model-status-filter").value || "all";

  const filtered = catalog.filter((m) => {
    if (statusFilter === "enabled" && m.enabled !== true) return false;
    if (statusFilter === "disabled" && m.enabled !== false) return false;
    if (search) {
      const hay = `${m.model} ${m.name} ${m.release_date || ""} ${m.description || ""}`.toLowerCase();
      if (!hay.includes(search)) return false;
    }
    return true;
  });

  const tbody = byId("model-table-body");
  if (!filtered.length) {
    const td = el("td", "muted", "没有符合筛选条件的目录模型。");
    td.setAttribute("colspan", "7");
    tbody.replaceChildren(el("tr", "", td));
  } else {
    tbody.replaceChildren(
      ...filtered.map((m) => {
        const w = weatherMap[m.model];
        const wMeta = w ? (WEATHER_META[w.status] || WEATHER_META.idle) : WEATHER_META.idle;
        const speedStr = w?.decode_p50 != null ? `${w.decode_p50} tok/s` : "—";
        const ttftStr = w?.ttft_p50_ms != null ? `首字 ${fmtDuration(w.ttft_p50_ms)}` : "暂无近7日样本";
        return el(
          "tr",
          "",
          (() => {
            const subText = m.release_date ? `${m.release_date} · ${m.description || m.name}` : (m.description || m.name);
            return el("td", "", el("div", "cell-primary mono", m.model), el("div", "cell-sub", subText));
          })(),
          el(
            "td",
            "",
            el("span", `badge tone-${m.enabled === false ? "muted" : wMeta.tone}`, m.enabled === false ? "已禁用" : wMeta.label),
            el("div", "cell-sub num", w ? `7日 ${fmtInt(w.week_requests)} 次 · 过载 ${fmtInt(w.week_overload_count)}` : "暂无调用记录")
          ),
          el("td", "num", speedStr, el("div", "cell-sub", ttftStr)),
          el("td", "num", fmtModelPrice(m.price?.input_usd_per_million)),
          el("td", "num", fmtModelPrice(m.price?.cached_input_usd_per_million)),
          el("td", "num", fmtModelPrice(m.price?.output_usd_per_million)),
          el("td", "num", `${fmtInt(m.route_count)} 条`)
        );
      })
    );
  }

  const unlisted = snapshot?.catalog_unlisted_count || 0;
  const note = byId("catalog-unlisted-note");
  note.hidden = unlisted <= 0;
  if (unlisted > 0) note.textContent = `另有 ${fmtInt(unlisted)} 个未公开展示的模型未在此目录列出。`;
}

/* ---------- Render: View 7 (#speed) Speed & Benchmark ---------- */

function renderSpeedView() {
  const speed = snapshot?.speed;
  const weatherMap = Object.fromEntries((snapshot?.model_weather || []).map((w) => [w.model, w]));

  byId("speed-overview-kpis").replaceChildren(
    el("div", "kpi-card", el("span", "kpi-label", "7 日总测速样本"), el("span", "kpi-value num", fmtInt(speed?.samples || 0)), el("span", "kpi-sub num", `累计可见+推理输出 ${fmtTokens(speed?.output_tokens || 0)}`)),
    el("div", "kpi-card", el("span", "kpi-label", "全站解码中位 (P50)"), el("span", "kpi-value num tone-sky", speed?.decode_p50 != null ? `${speed.decode_p50} tok/s` : "—"), el("span", "kpi-sub", "剔除首包等待与思考延迟后的纯净吐字速率")),
    el("div", "kpi-card", el("span", "kpi-label", "全站解码高端 (P90)"), el("span", "kpi-value num tone-emerald", speed?.decode_p90 != null ? `${speed.decode_p90} tok/s` : "—"), el("span", "kpi-sub", "短提示词轻量推理常态速度")),
    el("div", "kpi-card", el("span", "kpi-label", "端到端吞吐 P50"), el("span", "kpi-value num", speed?.e2e_p50 != null ? `${speed.e2e_p50} tok/s` : "—"), el("span", "kpi-sub", "包含长思考与首字时间在内的全流程速率"))
  );

  const models = speed?.models || [];
  byId("speed-table-body").replaceChildren(
    ...models.map((m) =>
      el(
        "tr",
        "",
        el("td", "mono cell-primary", m.model),
        el("td", "num", fmtInt(m.samples)),
        el("td", "num", m.decode_p50 != null ? `${m.decode_p50} tok/s` : "—"),
        el("td", "num", m.decode_p90 != null ? `${m.decode_p90} tok/s` : "—"),
        el("td", "num", m.e2e_p50 != null ? `${m.e2e_p50} tok/s` : "—"),
        el("td", "num", fmtDuration(weatherMap[m.model]?.ttft_p50_ms))
      )
    )
  );

  const speedSelect = byId("speed-model");
  const prevVal = speedSelect.value;
  const catalogEnabled = (snapshot?.model_catalog || [])
    .filter((m) => m.enabled !== false && m.model !== "codex-auto-review")
    .map((m) => m.model);
  const options = catalogEnabled.length ? catalogEnabled : ["gpt-6.2-astra", "gpt-6.2-sol", "gpt-6.1-sol"];
  speedSelect.replaceChildren(
    ...options.map((slug) => {
      const opt = el("option", "", slug);
      opt.value = slug;
      return opt;
    })
  );
  if (prevVal && options.includes(prevVal)) speedSelect.value = prevVal;
}

/* ---------- Master View Dispatcher & Networking ---------- */

function renderActiveView() {
  if (!snapshot) return;
  const sysFails = snapshot.request_health?.today?.system_failed_count ?? snapshot.request_health?.today?.failed ?? 0;
  const badge = byId("nav-errors-badge");
  badge.textContent = String(sysFails);
  badge.hidden = sysFails === 0;

  if (activeView === "overview") {
    renderPersonalCabin();
    renderPoolCabin();
    renderPulseStrip();
    renderTrendChart();
    renderModelWeatherBoard();
    renderOverviewErrorDigest();
  } else if (activeView === "errors") {
    renderErrorsView();
  } else if (activeView === "keys") {
    renderKeysView();
  } else if (activeView === "accounts") {
    renderAccountsView();
  } else if (activeView === "usage") {
    renderUsageView();
  } else if (activeView === "models") {
    renderModelsView();
  } else if (activeView === "speed") {
    renderSpeedView();
  }
}

function updateLiveUI() {
  const summary = byId("live-summary");
  const dot = byId("live-dot");
  if (!liveState || !liveState.available) {
    summary.textContent = "实时并发 暂不可读";
    dot.setAttribute("class", "live-dot is-offline");
    return;
  }
  const cap = Math.max(5, ((snapshot?.accounts || []).length || 3) * 5);
  summary.textContent = `实时总并发 ${liveState.total} / ${cap}`;
  dot.setAttribute("class", liveState.total >= cap - 2 ? "live-dot is-busy" : "live-dot");
  if (activeView === "overview") renderPoolCabin();
  if (activeView === "accounts") renderAccountsView();
}

function applySnapshotResponse(payload) {
  snapshot = payload.data;
  const age = Math.max(0, Number(payload.age_seconds || 0));
  const pill = byId("freshness-pill");
  const banner = byId("stale-banner");
  pill.textContent = payload.stale ? `延迟 ${Math.round(age / 60)}m` : `已同步 · ${age}s 前`;
  banner.hidden = !payload.stale;
  if (!selectedErrorDate) {
    selectedErrorDate = latestHealthDate();
  }
  syncMemberSelectors();
  renderActiveView();
}

async function loadDashboard() {
  const gen = authGeneration;
  try {
    const res = await fetch("/api/dashboard", { credentials: "same-origin", cache: "no-store" });
    if (gen !== authGeneration || !csrfToken) return;
    if (res.status === 401) {
      showLogin();
      return;
    }
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const payload = await res.json();
    if (gen !== authGeneration || !csrfToken) return;
    applySnapshotResponse(payload);
  } catch (err) {
    if (gen !== authGeneration) return;
    const pill = byId("freshness-pill");
    if (pill) pill.textContent = "拉取失败";
  }
}

async function loadLive() {
  const gen = authGeneration;
  if (!csrfToken) return;
  try {
    const res = await fetch("/api/live", { credentials: "same-origin", cache: "no-store" });
    if (gen !== authGeneration || !csrfToken) return;
    if (!res.ok) return;
    liveState = await res.json();
    updateLiveUI();
  } catch {
    // Keep last state silently
  }
}

function startPolling() {
  stopPolling();
  liveTimer = setInterval(() => {
    if (!document.hidden) loadLive();
  }, 5000);
  refreshTimer = setInterval(() => {
    if (!document.hidden) loadDashboard();
  }, 60000);
}

function stopPolling() {
  if (liveTimer) { clearInterval(liveTimer); liveTimer = null; }
  if (refreshTimer) { clearInterval(refreshTimer); refreshTimer = null; }
}

function showApp(csrf) {
  csrfToken = csrf;
  byId("login-panel").hidden = true;
  byId("app-shell").hidden = false;
  const hashView = (location.hash || "").replace(/^#/, "");
  setView(VALID_VIEWS.has(hashView) ? hashView : "overview", { pushHash: false });
  loadDashboard();
  loadLive();
  startPolling();
}

function showLogin() {
  authGeneration += 1;
  csrfToken = null;
  snapshot = null;
  liveState = null;
  stopPolling();
  const pw = byId("password");
  if (pw) pw.value = "";
  const speedKey = byId("speed-api-key");
  if (speedKey) speedKey.value = "";
  const errEl = byId("login-error");
  if (errEl) errEl.textContent = "";
  for (const cid of [
    "personal-cabin-body",
    "pool-cabin-body",
    "pulse-strip",
    "overview-trend-chart",
    "overview-model-weather",
    "overview-error-digest",
    "errors-signature-list",
    "failure-table-body",
    "keys-table-body",
    "accounts-card-grid",
    "model-table-body",
    "speed-table-body",
  ]) {
    const node = byId(cid);
    if (node) node.replaceChildren();
  }
  byId("drawer-root").hidden = true;
  byId("terminal-modal").hidden = true;
  byId("app-shell").hidden = true;
  byId("login-panel").hidden = false;
}

async function handleLogin(ev) {
  ev.preventDefault();
  const btn = byId("login-button");
  const errEl = byId("login-error");
  const username = byId("username").value;
  const password = byId("password").value;
  errEl.textContent = "";
  btn.disabled = true;
  try {
    const res = await fetch("/api/login", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      errEl.textContent = res.status === 429 ? "尝试过多，请 1 分钟后再试" : "账号或密码错误";
      return;
    }
    byId("password").value = "";
    authGeneration += 1;
    showApp(body.csrf);
  } catch {
    errEl.textContent = "网络异常，无法连接看板服务";
  } finally {
    btn.disabled = false;
  }
}

async function handleLogout() {
  const token = csrfToken;
  showLogin();
  if (!token) return;
  try {
    await fetch("/api/logout", {
      method: "POST",
      credentials: "same-origin",
      headers: { "X-CSRF-Token": token },
    });
  } catch {
    // Session cleared locally regardless.
  }
}

async function handleSpeedtest(ev) {
  ev.preventDefault();
  if (pendingSpeedtest || !csrfToken) return;
  const apiKey = byId("speed-api-key").value.trim();
  const model = byId("speed-model").value;
  const prompt = byId("speed-prompt").value.trim();
  const resultBox = byId("speedtest-result");
  const btn = byId("speed-submit-btn");

  pendingSpeedtest = true;
  btn.disabled = true;
  btn.textContent = "正在实测推理流…";
  resultBox.hidden = false;
  resultBox.replaceChildren(el("span", "muted", "已向网关发起短流式测试，请稍候…"));

  try {
    const res = await fetch("/api/speedtest", {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json", "X-CSRF-Token": csrfToken },
      body: JSON.stringify({ api_key: apiKey, model, prompt }),
    });
    const body = await res.json().catch(() => ({}));
    if (!res.ok) {
      resultBox.replaceChildren(el("span", "badge tone-rose", `测速失败 HTTP ${res.status}`), el("div", "cell-sub mono", body.detail || "请检查 API Key 或所选模型"));
      return;
    }
    resultBox.replaceChildren(
      el(
        "div",
        "acct-facts",
        el("div", "acct-fact", el("span", "", "可见解码速率"), el("strong", "num tone-sky", `${body.decode_tps} tok/s`)),
        el("div", "acct-fact", el("span", "", "首字延迟 TTFT"), el("strong", "num", fmtDuration(body.first_delta_ms))),
        el("div", "acct-fact", el("span", "", "总耗时 / 端到端"), el("strong", "num", `${fmtDuration(body.duration_ms)} (${body.e2e_tps} tok/s)`)),
        el("div", "acct-fact", el("span", "", "输出 / 可见 Token"), el("strong", "num", `${fmtInt(body.output_tokens)} / ${fmtInt(body.visible_tokens)}`))
      ),
      el("div", "speedtest-preview", body.text || "(空文本输出)")
    );
  } catch {
    resultBox.replaceChildren(el("span", "badge tone-rose", "测速请求网络异常"));
  } finally {
    pendingSpeedtest = false;
    btn.disabled = false;
    btn.textContent = "开始实时流式测速";
  }
}


function copySnippetText(text, btn) {
  const buf = byId("terminal-copy-buffer");
  buf.value = text;
  if (typeof buf.select === "function") buf.select();
  let copied = false;
  if (typeof document.execCommand === "function") {
    try {
      copied = Boolean(document.execCommand("copy"));
    } catch {
      copied = false;
    }
  }
  btn.textContent = copied ? "已复制" : "已选中";
  setTimeout(() => {
    btn.textContent = "复制命令";
  }, 1600);
}

function buildTerminalPreviewText(selected, nowSec) {
  const label = selected ? selected.label : "示例成员";
  const status = selected ? String(selected.status || "active").toUpperCase() : "ACTIVE";
  const limitCents = selected ? (selected.weekly_limit_cents || 24000) : 24000;
  const usedCents = selected ? clamp(selected.weekly_used_cents || 0, 0, limitCents) : 5360;
  const remCents = Math.max(0, limitCents - usedCents);
  const pct = limitCents > 0 ? Math.min(100, (usedCents * 100) / limitCents) : 0;
  const filled = Math.min(32, Math.max(0, Math.round((32 * pct) / 100)));
  const bar = "█".repeat(filled) + "░".repeat(32 - filled);
  const nextStr = selected && usedCents > 0 && selected.next_release_at
    ? `${selected.next_release_cents > 0 ? `+${fmtCents(selected.next_release_cents)} ` : ""}in ${fmtCountdown(selected.next_release_at, nowSec)} · ${fmtTime(selected.next_release_at)}`
    : usedCents > 0
      ? "+$12.50 in 2h 30m · 2026-11-15 10:30"
      : "none (no billable usage in window)";
  const padHeader = " ".repeat(60 - 11 - 10);
  const rule = "─".repeat(60);
  const statuslinePct = ((remCents * 100) / limitCents).toFixed(1);
  const statusline = `$${(remCents / 100).toFixed(2)}/$${Math.round(limitCents / 100)} left (${statuslinePct}%)` +
    (usedCents > 0 && (selected?.next_release_at || !selected) ? " · next +$12.50 in 2h 30m" : "");
  const card = [
    `  GPT USAGE${padHeader}rolling 7d`,
    `  ${rule}`,
    `  MEMBER      ${label}`,
    `  STATUS      ${status}`,
    "",
    `  $${(remCents / 100).toFixed(2)}  remaining`,
    `  $${(usedCents / 100).toFixed(2)} used of $${(limitCents / 100).toFixed(2)}  |  ${pct.toFixed(1)}% of window`,
    `  ${bar}`,
    "",
    `  NEXT FREE   ${nextStr}`,
    `  WINDOW      rolling 7 days`,
    `  ${rule}`,
  ].join("\n");
  return { card, statusline };
}

function buildSnippetBlock(title, note, codeText) {
  const copyBtn = el("button", "btn subtle small", "复制命令");
  copyBtn.type = "button";
  copyBtn.addEventListener("click", () => copySnippetText(codeText, copyBtn));
  const pre = el("pre", "snippet-pre", codeText);
  return el(
    "section",
    "snippet-card",
    el("div", "snippet-head", el("h3", "snippet-title", title), copyBtn),
    el("p", "snippet-note", note),
    pre
  );
}

function renderTerminalModal() {
  const body = byId("terminal-modal-body");
  if (!body) return;
  const selected = findSelectedKey();
  const terminalOrigins = resolveTerminalOrigins();
  const originCfg = terminalOrigins[terminalOriginMode] || terminalOrigins.public;
  const originUrl = originCfg.url;
  const nowSec = Math.floor(Date.now() / 1000);
  const preview = buildTerminalPreviewText(selected, nowSec);

  const pubBtn = el("button", `origin-btn${terminalOriginMode === "public" ? " is-active" : ""}`, terminalOrigins.public.label);
  pubBtn.id = "terminal-origin-public";
  pubBtn.type = "button";
  pubBtn.addEventListener("click", () => {
    terminalOriginMode = "public";
    renderTerminalModal();
  });

  const lanBtn = el("button", `origin-btn${terminalOriginMode === "intranet" ? " is-active" : ""}`, terminalOrigins.intranet.label);
  lanBtn.id = "terminal-origin-intranet";
  lanBtn.type = "button";
  lanBtn.addEventListener("click", () => {
    terminalOriginMode = "intranet";
    renderTerminalModal();
  });

  const whoBadge = el(
    "span",
    selected ? "status-pill tone-emerald" : "status-pill tone-sky",
    selected ? `当前成员预览：${selected.label} · 剩余 ${fmtCents(selected.weekly_remaining_cents ?? 24000)}` : "未选成员视角（显示示例卡片，选择右上角姓名即实时预览）"
  );

  const toolbar = el(
    "div",
    "terminal-toolbar",
    el("div", "origin-switch", pubBtn, lanBtn),
    whoBadge
  );

  const previewSection = el(
    "section",
    "snippet-card",
    el("div", "snippet-head", el("h3", "snippet-title", "实时卡片与 Statusline 效果预览"), el("span", "badge tone-emerald", "0 Token · $0.00")),
    el("pre", "terminal-card-preview", preview.card),
    el("div", "terminal-statusline-preview", `Statusline: ${preview.statusline}`)
  );

  const curlSnippet = [
    `export TEAM_GATEWAY_ORIGIN="${originUrl}"`,
    `export TEAM_API_KEY="在这里替换为你的专属API_KEY"`,
    "",
    `# 可视化卡片`,
    `curl -sS -H "Authorization: Bearer $TEAM_API_KEY" "$TEAM_GATEWAY_ORIGIN/v1/usage?pretty=1"`,
    "",
    `# 一键别名（写入 ~/.zshrc 或 ~/.bashrc 后随时输入 gq 查看）`,
    `alias gq='curl -sS -H "Authorization: Bearer $TEAM_API_KEY" "$TEAM_GATEWAY_ORIGIN/v1/usage?pretty=1"'`,
  ].join("\n");

  const chatSnippet = [
    `# 在 Codex CLI 或 Claude Code 对话框直接输入以下任一指令（网关本地秒回，0 Token / $0.00）：`,
    `?usage`,
    `?quota`,
    `#usage`,
    `查额度`,
    "",
    `# 重要：Codex CLI 和 Claude Code 都会在客户端本地拦截内置的 /usage（Codex 还会拦截 /quota），`,
    `# 不会发往中转站；请直接发 "?usage"、"?quota"、"#usage" 或 "查额度"。`,
    `# 已适配 Claude Code 2.x 自动附带的 <system-reminder> 与末尾 # Environment 系统块；`,
    `# 同时支持 Codex 新版 WebSocket 通道与 HTTP /v1/responses，且 Key 用满 $240 后依然可查。`,
    `# 每次响应头也会自动替换为本 Key 的 x-codex-* 滚动配额信息。`,
  ].join("\n");

  const claudeSnippet = [
    `# 1) Claude Code 环境变量（无需改 ~/.claude/settings.json 的也可放进 shell；ANTHROPIC_BASE_URL 不要带末尾 /v1）：`,
    `export ANTHROPIC_BASE_URL="${originUrl}"`,
    `export ANTHROPIC_AUTH_TOKEN="在这里替换为你的专属API_KEY"`,
    `export ANTHROPIC_MODEL="gpt-6-astra[1m]"         # [1m] 可消除 Claude Code 的 200k 上下文告警`,
    `export ANTHROPIC_DEFAULT_HAIKU_MODEL="gpt-6-luna" # 探索子代理默认更省钱；未设置时网关也会自动映射 claude-*`,
    "",
    `# 2) 安装底部 Statusline 与 /quota 自定义斜杠指令：`,
    `mkdir -p ~/.claude/commands && cat <<'SH' > ~/.claude/team-quota-statusline.sh && chmod +x ~/.claude/team-quota-statusline.sh`,
    `#!/usr/bin/env bash`,
    `KEY="\${ANTHROPIC_AUTH_TOKEN:-\${ANTHROPIC_API_KEY:-}}"`,
    `BASE="\${ANTHROPIC_BASE_URL:-${originUrl}}"`,
    `BASE="\${BASE%/v1}"`,
    `[ -n "$KEY" ] && curl -fsS -m 2 -H "x-api-key: $KEY" "$BASE/v1/usage?format=statusline" 2>/dev/null || echo "Key $240"`,
    `SH`,
    "",
    `cat <<'MD' > ~/.claude/commands/quota.md`,
    `直接在终端执行只读命令 \`BASE="\${ANTHROPIC_BASE_URL:-${originUrl}}"; curl -sS -H "x-api-key: \${ANTHROPIC_AUTH_TOKEN:-\${ANTHROPIC_API_KEY}}" "\${BASE%/v1}/v1/usage?pretty=1"\` 并原样展示输出结果（不要调用任何其他工具或修改文件）。`,
    `MD`,
    `# 在 ~/.claude/settings.json 中添加: "statusLine": { "type": "command", "command": "~/.claude/team-quota-statusline.sh" }`,
  ].join("\n");

  body.replaceChildren(
    toolbar,
    previewSection,
    buildSnippetBlock("方式 1 · 对话内直接发送（零配置 · Codex CLI & Claude Code 通用）", "本地拦截响应耗时 <1ms，计费日志固定 0 Token / $0.00，甚至 Key 用满 $240 后依然可查。", chatSnippet),
    buildSnippetBlock("方式 2 · 终端 curl / gq 快捷命令（零消耗 · JSON / 卡片 / Statusline）", "公网与内网入口均统一走 /v1/usage，支持 ?pretty=1 / ?format=statusline / 默认 JSON 三种格式。", curlSnippet),
    buildSnippetBlock("方式 3 · Claude Code 推荐配置 + 底部 Statusline + /quota 指令", "同一把团队 Key 直接兼容 Claude Code，网关自动适配 Haiku/Sonnet/Opus 子代理与长上下文后缀。", claudeSnippet)
  );
}

function openTerminalModal() {
  const modal = byId("terminal-modal");
  if (!modal) return;
  modal.hidden = false;
  renderTerminalModal();
}

function closeTerminalModal() {
  const modal = byId("terminal-modal");
  if (modal) modal.hidden = true;
}

async function bootstrap() {
  selectedKeyId = readStorage(STORAGE_MEMBER_KEY, "");
  renderNavIcons();
  byId("login-form").addEventListener("submit", handleLogin);
  byId("logout-button").addEventListener("click", handleLogout);
  byId("refresh-button").addEventListener("click", () => { loadDashboard(); loadLive(); });

  const togglePw = byId("toggle-password");
  if (togglePw) {
    togglePw.addEventListener("click", () => {
      const pw = byId("password");
      const show = pw.type === "password";
      pw.type = show ? "text" : "password";
      togglePw.textContent = show ? "隐藏" : "显示";
    });
  }

  for (const btn of document.querySelectorAll("[data-view]")) {
    btn.addEventListener("click", () => setView(btn.dataset.view));
  }
  for (const btn of document.querySelectorAll("[data-period]")) {
    btn.addEventListener("click", () => {
      activePeriod = btn.dataset.period;
      renderUsageView();
    });
  }

  byId("global-member-select").addEventListener("change", (ev) => selectMember(ev.target.value));
  byId("overview-member-select").addEventListener("change", (ev) => selectMember(ev.target.value));
  byId("jump-errors-btn").addEventListener("click", () => {
    if (overviewErrorTab === "yesterday" && (snapshot?.request_health?.days?.length || 0) >= 2) {
      const days = snapshot.request_health.days;
      selectedErrorDate = days[days.length - 2].date;
    } else if (overviewErrorTab === "week") {
      selectedErrorDate = "week";
    } else {
      selectedErrorDate = latestHealthDate();
    }
    setView("errors");
  });
  byId("jump-models-btn").addEventListener("click", () => setView("models"));

  byId("errors-scope-week-btn").addEventListener("click", () => {
    selectedErrorDate = selectedErrorDate === "week" ? latestHealthDate() : "week";
    renderErrorsView();
  });
  byId("failure-search").addEventListener("input", () => renderErrorsView());
  byId("failure-layer-filter").addEventListener("change", (ev) => {
    selectedLayerFilter = ev.target.value;
    renderErrorsView();
  });
  byId("failure-my-only-btn").addEventListener("click", () => {
    onlySelectedMemberFailures = !onlySelectedMemberFailures;
    renderErrorsView();
  });

  byId("key-search").addEventListener("input", () => renderKeysView());
  byId("key-status-filter").addEventListener("change", () => renderKeysView());
  byId("key-sort").addEventListener("change", () => renderKeysView());
  byId("model-search").addEventListener("input", () => renderModelsView());
  byId("model-status-filter").addEventListener("change", () => renderModelsView());
  byId("speedtest-form").addEventListener("submit", handleSpeedtest);

  byId("terminal-guide-button").addEventListener("click", openTerminalModal);
  byId("terminal-modal-close").addEventListener("click", closeTerminalModal);
  byId("terminal-modal-backdrop").addEventListener("click", closeTerminalModal);
  byId("drawer-close").addEventListener("click", () => { byId("drawer-root").hidden = true; });
  byId("drawer-backdrop").addEventListener("click", () => { byId("drawer-root").hidden = true; });
  window.addEventListener("keydown", (ev) => {
    if (ev.key !== "Escape") return;
    if (!byId("terminal-modal").hidden) closeTerminalModal();
    if (!byId("drawer-root").hidden) byId("drawer-root").hidden = true;
  });
  window.addEventListener("hashchange", () => {
    const hashView = (location.hash || "").replace(/^#/, "");
    if (VALID_VIEWS.has(hashView)) setView(hashView, { pushHash: false });
  });

  try {
    const res = await fetch("/api/session", { credentials: "same-origin", cache: "no-store" });
    if (res.ok) {
      const body = await res.json();
      showApp(body.csrf);
      return;
    }
  } catch {
    // Fall back cleanly to login screen
  }
  showLogin();
}

if (typeof document !== "undefined" && !globalThis.__DASHBOARD_TEST__) {
  bootstrap();
}
