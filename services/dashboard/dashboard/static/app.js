"use strict";
const $ = (id) => document.getElementById(id);
let csrf = "", snapshot = null, period = "today", view = "overview", fetching = false, sessionVersion = 0;
let countdowns = [];
const fmt = new Intl.NumberFormat("zh-CN");
const number = (value) => fmt.format(value || 0);
const compact = (value) => new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 2 }).format(value || 0);
const money = (value) => "$" + Number(value || 0).toFixed(3);
const moneyFine = (value) => "$" + Number(value || 0).toFixed(4);
const date = (value) => value ? new Date(value * 1000).toLocaleString("zh-CN", { timeZone: "Asia/Shanghai", hour12: false }) : "暂无记录";
// Fixed interface geometry only. No server-provided SVG or HTML is accepted.
const ICONS = {
  home: ["m3 10 9-7 9 7", "M5 9v12h14V9", "M9 21v-8h6v8"],
  users: ["M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2", "M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8", "M17 3a4 4 0 0 1 0 8", "M22 21v-2a4 4 0 0 0-3-3.87"],
  key: ["M15.5 12a5.5 5.5 0 1 0-5.4-4.5L2 16v6h6v-3h3v-3l1.5-1.5", "M16 7h.01"],
  cube: ["m12 3 9 5v8l-9 5-9-5V8z", "m3 8 9 5 9-5", "M12 13v8", "m7.5 5.5 9 5"],
  lock: ["M5 10h14v11H5z", "M8 10V7a4 4 0 0 1 8 0v3", "M12 15v2"],
  refresh: ["M20 7v5h-5", "M4 17v-5h5", "M5.5 7a8 8 0 0 1 13-2L20 7", "M4 17l1.5 2a8 8 0 0 0 13-2"],
  check: ["M22 11.1V12a10 10 0 1 1-5.9-9.1", "m9 11 3 3L22 4"],
  activity: ["M2 12h4l3-9 6 18 3-9h4"],
  bolt: ["m13 2-9 12h7l-1 8 10-12h-7z"],
  database: ["M3 5c0-4 18-4 18 0s-18 4-18 0", "M3 5v14c0 4 18 4 18 0V5", "M3 12c0 4 18 4 18 0"],
  dollar: ["M12 2v20", "M17 5H9a4 4 0 0 0 0 8h6a4 4 0 0 1 0 8H5"]
};
function node(tag, text, className) {
  const element = document.createElement(tag);
  if (text !== undefined) element.textContent = text;
  if (className) element.className = className;
  return element;
}
function icon(name) {
  const wrapper = node("span", undefined, "icon"), svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24"); wrapper.setAttribute("aria-hidden", "true");
  for (const path of ICONS[name] || ICONS.lock) { const p = document.createElementNS("http://www.w3.org/2000/svg", "path"); p.setAttribute("d", path); svg.append(p); }
  wrapper.append(svg); return wrapper;
}
document.querySelectorAll("[data-icon]").forEach((el) => el.append(icon(el.dataset.icon).firstChild));
function navigate(next, changeHash = true) {
  view = ["overview", "accounts", "claude-accounts", "apikeys", "models"].includes(next) ? next : "overview";
  for (const name of ["overview", "accounts", "claude-accounts", "apikeys", "models"]) $("view-" + name).hidden = name !== view;
  $("page-title").textContent = { overview: "仪表盘", accounts: "OpenAI 账号池", "claude-accounts": "Claude 账号池", apikeys: "平台密钥", models: "模型目录" }[view];
  $("period-controls").hidden = view === "models";
  document.querySelectorAll("nav [data-view]").forEach((link) => {
    if (link.dataset.view === view) link.setAttribute("aria-current", "page"); else link.removeAttribute("aria-current");
  });
  if (changeHash && location.hash !== "#" + view) history.replaceState(null, "", "#" + view);
}
function showLogin(message = "") {
  sessionVersion++; csrf = ""; snapshot = null; countdowns = []; $("dashboard").hidden = true; $("login-panel").hidden = false;
  $("login-error").textContent = message;
  for (const id of ["accounts", "claude-accounts", "overview-accounts", "keys", "key-groups", "models", "catalog-rows"]) $(id).replaceChildren();
}
async function api(path, options = {}) {
  const response = await fetch(path, { cache: "no-store", credentials: "same-origin", ...options });
  if (!response.ok) { const error = new Error("request_failed"); error.status = response.status; throw error; }
  return response.json();
}
function status(value) {
  return node("span", ({ enabled: "启用", disabled: "停用", needs_login: "需重新登录", unavailable: "不可用", unknown: "未知" })[value] || "未知", "status " + value);
}
function windowLabel(minutes, fallback) {
  if (!minutes) return fallback;
  if (minutes === 10080) return "周额度";
  if (minutes % 1440 === 0) return (minutes / 1440) + " 天";
  if (minutes % 60 === 0) return (minutes / 60) + " 小时";
  return minutes + " 分钟";
}
function countdownText(resetsAt, now = Date.now() / 1000) {
  if (!resetsAt) return "等待更新";
  if (resetsAt <= now) return "等待额度更新";
  const minutes = Math.ceil((resetsAt - now) / 60), days = Math.floor(minutes / 1440), hours = Math.floor(minutes % 1440 / 60);
  return (days ? days + " 天 " : "") + (hours ? hours + " 时 " : "") + (minutes % 60) + " 分后重置";
}
function renderWindow(data, fallback, capturedAt, extra = false) {
  data = data || {};
  const section = node("div", undefined, extra ? "window spark-window" : "window"), heading = node("div", undefined, "window-heading");
  const label = extra || !data.minutes ? fallback : windowLabel(data.minutes, fallback);
  heading.append(node("span", label), node("strong", data.remaining_percent == null ? "暂无数据" : data.remaining_percent.toFixed(1) + "%"));
  section.append(heading);
  if (data.remaining_percent != null) {
    const progress = node("progress"); progress.max = 100; progress.value = data.remaining_percent;
    progress.setAttribute("aria-label", label + "剩余额度"); section.append(progress);
  } else {
    section.append(node("div", undefined, "unknown-track"));
  }
  const old = capturedAt && Date.now() / 1000 - capturedAt > 600;
  if (data.resets_at) {
    section.append(node("small", date(data.resets_at)));
    const countdown = node("small", countdownText(data.resets_at), "reset-countdown"); section.append(countdown);
    countdowns.push({ element: countdown, resetsAt: data.resets_at });
  } else if (data.remaining_percent != null) section.append(node("small", "重置时间待更新"));
  if (old) section.append(node("small", "数据待更新", "quota-stale"));
  return section;
}
function primaryWindows(account) {
  const primary = account.primary || {}, secondary = account.secondary || {};
  // A Pro weekly window can arrive in either slot. Do not call it a 5h window.
  if (primary.minutes > 1440 && (!secondary.minutes || secondary.minutes <= 1440)) {
    return [secondary, primary];
  }
  return [primary, secondary];
}
function isWeeklyOnlyPro(account) {
  const [short, long] = primaryWindows(account);
  // Apply the user's confirmed weekly-only Pro setup narrowly; missing data
  // for other plans (or both windows) never establishes unlimited quota.
  return Boolean(account.plan === "pro" && account.captured_at && long.minutes === 10080 &&
    long.remaining_percent != null && short.remaining_percent == null && !short.minutes && !short.resets_at);
}
function accountWindows(account, includeSpark = false) {
  const windows = node("div", undefined, "windows"), [short, long] = primaryWindows(account);
  if (isWeeklyOnlyPro(account)) {
    const section = node("div", undefined, "window window-unlimited"), heading = node("div", undefined, "window-heading");
    heading.append(node("span", "5 小时"), node("strong", "不单独限额"));
    section.append(heading, node("small", "按周额度使用")); windows.append(section);
  } else windows.append(renderWindow(short, "5 小时", account.captured_at));
  windows.append(renderWindow(long, "7 天", account.captured_at));
  if (includeSpark) windows.append(renderWindow(account.spark_primary, "Spark 额度", account.captured_at, true),
    renderWindow(account.spark_secondary, "Spark 额度 · 长周期", account.captured_at, true));
  return windows;
}
function accountStatus(account) {
  const badge = status(account.status);
  if (account.status !== "enabled") return badge;
  const [short, long] = primaryWindows(account);
  if (short.remaining_percent === 0 || long.remaining_percent === 0) { badge.textContent = "额度已用尽"; badge.className = "status unavailable"; }
  else if (isWeeklyOnlyPro(account)) badge.textContent = "仅 7 天额度";
  else if (short.remaining_percent == null && long.remaining_percent == null) { badge.textContent = "额度未知"; badge.className = "status unknown"; }
  return badge;
}
function accountCard(account) {
  const card = node("article", undefined, "account"), head = node("div", undefined, "account-head"), title = node("div", undefined, "account-title");
  const avatar = node("span", undefined, "account-avatar"); avatar.append(icon("users"));
  const label = node("div", account.label, "account-label");
  label.append(node("span", account.plan === "unknown" ? "套餐未知" : account.plan.toUpperCase(), "account-plan"));
  label.append(node("span", account.group_name || "未分组", "group-badge"));
  title.append(avatar, label); head.append(title, accountStatus(account)); card.append(head);
  card.append(accountWindows(account), node("p", "最近更新：" + date(account.captured_at), "account-time")); return card;
}
function claudeAccountCard(account) {
  const card = node("article", undefined, "account"), head = node("div", undefined, "account-head"), title = node("div", undefined, "account-title");
  const avatar = node("span", undefined, "account-avatar claude-avatar"); avatar.append(icon("users"));
  const label = node("div", account.email || account.label, "account-label");
  label.append(node("span", "Claude · " + (account.plan === "unknown" ? "套餐未知" : account.plan.toUpperCase()), "account-plan"));
  title.append(avatar, label); head.append(title, status(account.status)); card.append(head);
  card.append(node("p", "订阅额度暂不可查", "claude-quota-note"), node("p", "最近更新：" + date(account.updated_at), "account-time"));
  return card;
}
function currentKeys() {
  // Also handle snapshots written by an older collector during deployment.
  return snapshot.keys.filter((key) => key.is_historical !== true);
}
function renderAccounts() {
  countdowns = [];
  // The server validates old snapshots and supplies an empty default list;
  // pool_usage is the collector-version marker that old snapshots lack.
  const hasClaudeData = Array.isArray(snapshot.claude_accounts) && snapshot.pool_usage != null,
    claudeAccounts = hasClaudeData ? snapshot.claude_accounts : [];
  $("account-count").textContent = "共 " + snapshot.accounts.length + " 个账号";
  $("claude-account-count").textContent = hasClaudeData ? "共 " + claudeAccounts.length + " 个账号" : "账号数量待更新";
  $("overview-account-count").textContent = hasClaudeData ? number(snapshot.accounts.length + claudeAccounts.length) : "—";
  $("overview-enabled-count").textContent = hasClaudeData ? number([...snapshot.accounts, ...claudeAccounts].filter((a) => a.status === "enabled").length) : "—";
  $("overview-key-count").textContent = number(currentKeys().length);
  $("accounts").replaceChildren(); $("claude-accounts").replaceChildren(); $("overview-accounts").replaceChildren();
  const highlighted = snapshot.accounts.slice(0, claudeAccounts.length ? 1 : 2);
  for (const account of highlighted) $("overview-accounts").append(accountCard(account));
  if (claudeAccounts.length) $("overview-accounts").append(claudeAccountCard(claudeAccounts[0]));
  if (snapshot.accounts.length + claudeAccounts.length > highlighted.length + Math.min(claudeAccounts.length, 1))
    $("overview-accounts").append(node("p", "其余账号请在对应账号池中查看。", "muted"));
  if (!snapshot.accounts.length && !claudeAccounts.length) $("overview-accounts").append(node("div", hasClaudeData ? "暂无可展示账号。" : "账号统计待更新。", "empty"));
  if (!snapshot.accounts.length) {
    const row = node("tr"), cell = node("td", "暂无可展示账号。"); cell.colSpan = 6; row.append(cell); $("accounts").append(row);
  }
  for (const account of snapshot.accounts) {
    const row = node("tr"), name = node("td"), quota = node("td"), state = node("td"), operation = node("td");
    const title = node("div", account.email || account.label, "account-label"); title.append(node("span", account.plan === "unknown" ? "套餐未知" : account.plan.toUpperCase(), "account-plan")); name.append(title);
    if (account.email) name.append(node("div", account.label, "account-alias"));
    name.append(node("div", "分组：" + (account.group_name || "未分组"), "group-badge"));
    name.append(node("div", "最近刷新：" + date(account.captured_at), "account-detail"),
      node("div", "订阅到期：" + (account.subscription_expires_at ? date(account.subscription_expires_at) : "未提供"), "account-detail"));
    if (account.subscription_renews_at) name.append(node("div", "续订时间：" + date(account.subscription_renews_at), "account-detail"));
    quota.append(accountWindows(account, true));
    const badges = node("div", undefined, "quota-badges");
    if (account.capacity_override) {
      const capacity = node("span", "自定义容量", "quota-badge");
      capacity.title = "管理员设置的 Token 容量估计"; badges.append(capacity);
      badges.append(node("span", "5h：" + (account.capacity_primary_tokens == null ? "未设置" : number(account.capacity_primary_tokens)) +
        " / 7d：" + (account.capacity_secondary_tokens == null ? "未设置" : number(account.capacity_secondary_tokens)) + " Token（估计）", "capacity-detail"));
    }
    const resets = node("span", "可用重置次数：" + (account.reset_available_count == null ? "待更新" : number(account.reset_available_count)), "quota-badge reset-credit");
    resets.title = "以上方更新时间为准";
    badges.append(resets);
    if (account.reset_next_expires_at) badges.append(node("span", "最近重置券到期：" + date(account.reset_next_expires_at), "capacity-detail"));
    quota.append(badges);
    state.append(accountStatus(account));
    const proxy = node("td", ({ configured: "已启用", disabled: "未启用", inherited: "—", unknown: "未知" })[account.proxy_state || "unknown"]);
    proxy.title = "— 表示没有单独设置账号代理";
    const order = node("td"); order.append(node("span", account.sort_order == null ? "—" : String(account.sort_order), "sort-value"));
    const readonly = node("span", undefined, "readonly-cell"); readonly.append(icon("lock"), node("span", "仅查看")); operation.append(readonly);
    row.append(name, quota, order, proxy, state, operation); $("accounts").append(row);
  }
  if (!claudeAccounts.length) {
    const row = node("tr"), cell = node("td", hasClaudeData ? "暂无 Claude 订阅账号。" : "等待新版采集器统计。", "empty"); cell.colSpan = 6; row.append(cell); $("claude-accounts").append(row);
  }
  for (const account of claudeAccounts) {
    const row = node("tr"), name = node("td"), title = node("div", account.email || account.label, "account-label");
    name.append(title);
    if (account.email) name.append(node("div", account.label, "account-alias"));
    const plan = node("td"); plan.append(node("span", account.plan === "unknown" ? "套餐未知" : account.plan.toUpperCase(), "table-tag"));
    const order = node("td"); order.append(node("span", account.sort_order == null ? "—" : String(account.sort_order), "sort-value"));
    row.append(name, plan, order, node("td", undefined)); row.children[3].append(status(account.status));
    row.append(node("td", date(account.updated_at)), node("td", "暂无额度数据"));
    $("claude-accounts").append(row);
  }
}
function renderGroupUsage() {
  const body = $("key-groups"); body.replaceChildren();
  const groups = [...(snapshot.key_groups || [])].sort((a, b) => b.usage[period].total_tokens - a.usage[period].total_tokens);
  $("groups-period").textContent = ({ today: "今日", week: "近 7 天", recorded: "累计" })[period] + " · 按 Token 用量降序";
  if (!groups.length) {
    const hasUsage = currentKeys().length || Object.values(snapshot.totals[period]).some((value) => value > 0);
    const row = node("tr"), cell = node("td", hasUsage ? "分组统计待更新，请稍后刷新。" : "暂无分组用量。", "empty");
    cell.colSpan = 8; row.append(cell); body.append(row); return;
  }
  for (const group of groups) {
    const row = node("tr"), label = node("td"), usage = group.usage[period];
    label.append(node("div", group.label, "group-label"));
    if (group.kind === "unrestricted") label.append(node("small", "Key 未限制账号分组", "group-detail"));
    if (group.kind === "unattributed") label.append(node("small", "保留历史记录，用于核对总量", "group-detail"));
    row.append(label, node("td", group.kind === "unattributed" ? "—" : number(group.key_count)));
    for (const field of ["requests", "input_tokens", "cached_tokens", "output_tokens", "total_tokens"]) {
      row.append(node("td", number(usage[field]), field === "total_tokens" ? "group-total" : undefined));
    }
    const cost = node("td", moneyFine(usage.estimated_usd)); cost.title = "估算美元金额，仅供参考";
    row.append(cost); body.append(row);
  }
}
function renderPoolUsage() {
  const pool = snapshot.pool_usage;
  for (const [name, overviewId, usageId, requestsId] of [
    ["openai", "overview-openai-tokens", "openai-pool-usage", "openai-pool-requests"],
    ["claude", "overview-claude-tokens", "claude-pool-usage", "claude-pool-requests"],
    ["unattributed", "overview-unattributed-tokens", null, null]
  ]) {
    const value = pool && pool[name] && pool[name][period];
    $(overviewId).textContent = value ? compact(value.total_tokens) : "—";
    $(overviewId).title = value ? number(value.total_tokens) + " Token" : "等待新版采集器统计";
    if (usageId) $(usageId).textContent = value ? number(value.total_tokens) + " Token" : "统计待更新";
    if (requestsId) $(requestsId).textContent = value ? number(value.requests) + " 次请求 · 参考金额 " + money(value.estimated_usd) : "等待新版采集器统计";
  }
}
function renderUsage() {
  const total = snapshot.totals[period];
  for (const [id, field] of [["total-tokens", "total_tokens"], ["total-cached", "cached_tokens"], ["total-output", "output_tokens"]]) {
    $(id).textContent = compact(total[field]); $(id).title = number(total[field]);
  }
  $("total-requests").textContent = number(total.requests); $("total-cost").textContent = money(total.estimated_usd);
  $("keys-total-tokens").textContent = compact(total.total_tokens); $("keys-total-tokens").title = number(total.total_tokens);
  $("keys-total-cost").textContent = money(total.estimated_usd);
  renderPoolUsage();
  renderGroupUsage();
  $("keys").replaceChildren();
  // currentKeys returns a filtered copy; sorting never changes the snapshot.
  const keys = currentKeys().sort((a, b) => b.usage[period].total_tokens - a.usage[period].total_tokens);
  $("keys-sort-order").textContent = "按" + ({ today: "今日", week: "近 7 天", recorded: "累计" })[period] + " Token 用量从高到低排列";
  if (!keys.length) { const tr = node("tr"), td = node("td", "暂无密钥记录。"); td.colSpan = 9; tr.append(td); $("keys").append(tr); }
  for (const key of keys) {
    const usage = key.usage[period], row = node("tr"), identifier = node("td"), state = node("td"), operation = node("td"), provider = node("td"), protocol = node("td"), rotation = node("td"), tokens = node("td", undefined, "key-usage");
    const keyId = key.display_id || key.id;
    const keyName = typeof key.label === "string" ? key.label.trim() : "";
    identifier.className = "key-identity";
    identifier.append(node("div", keyName || keyId, "key-label"));
    if (keyName && keyName !== keyId) identifier.append(node("span", keyId, "key-id"));
    if (key.group_name !== undefined) identifier.append(node("div", key.group_name || "全部分组（未限制）", "group-badge"));
    identifier.title = "显示名称和编号，不是完整密钥";
    provider.append(node("span", ({ openai: "OpenAI", claude: "Claude" })[key.upstream_provider] || "未知", "table-tag"));
    protocol.append(node("span", ({ openai_compat: "OpenAI 兼容", anthropic_native: "Anthropic 原生", gemini_native: "Gemini 原生" })[key.protocol] || "未知协议", "table-tag"));
    rotation.append(node("span", ({ account_rotation: "账号轮转", aggregate_api_rotation: "聚合 API 轮转", hybrid_rotation: "混合轮转 · 账号优先", hybrid_aggregate_first_rotation: "混合轮转 · 聚合优先" })[key.rotation] || "未知", "table-tag"));
    const boundModel = ({ request: "跟随请求", fixed: key.bound_model, unlisted: "已绑定 · 模型未公开", unknown: "未知" })[key.model_binding || "unknown"];
    const today = key.usage.today, recorded = key.usage.recorded;
    tokens.append(node("small", "今日"), node("div", compact(today.total_tokens) + " · " + moneyFine(today.estimated_usd)));
    if (period === "week") tokens.append(node("small", "近 7 天"), node("div", compact(key.usage.week.total_tokens) + " · " + moneyFine(key.usage.week.estimated_usd)));
    tokens.append(node("small", "累计"), node("div", compact(recorded.total_tokens), "key-cumulative"));
    const quotaText = key.quota_limit_tokens == null ? (key.quota_config_known ? "不限额" : "限额未知") : "限额 " + compact(key.quota_limit_tokens) + " Token";
    tokens.append(node("small", "约 " + moneyFine(recorded.estimated_usd) + " · " + quotaText));
    if (key.quota_limit_tokens != null) tokens.append(node("small", "剩余 " + compact(Math.max(0, key.quota_limit_tokens - recorded.total_tokens)) + " Token", recorded.total_tokens >= key.quota_limit_tokens ? "limit-exhausted" : ""));
    const details = node("details", undefined, "usage-details");
    details.append(node("summary", "用量明细 · " + ({ today: "今日", week: "近 7 天", recorded: "累计" })[period]),
      node("div", "请求 " + number(usage.requests) + " · 输入 " + number(usage.input_tokens) + " · 缓存 " + number(usage.cached_tokens) + " · 输出 " + number(usage.output_tokens)));
    tokens.append(details); state.append(status(key.status));
    const readonly = node("span", undefined, "readonly-cell"); readonly.append(icon("lock"), node("span", "仅查看")); operation.append(readonly);
    row.append(identifier, provider, protocol, rotation, node("td", boundModel), node("td", key.last_used_at ? date(key.last_used_at) : "从未调用"), tokens, state, operation);
    $("keys").append(row);
  }
  document.querySelectorAll("[data-period]").forEach((button) => button.setAttribute("aria-pressed", String(button.dataset.period === period)));
}
function renderCatalog() {
  $("catalog-rows").replaceChildren();
  const items = snapshot.model_catalog || [], search = ($("catalog-search").value || "").trim().toLowerCase(), filter = $("catalog-status").value || "all";
  const visible = items.filter((item) => (filter === "all" || (filter === "enabled" ? item.enabled === true : item.enabled === false)) &&
    (!search || [item.model, item.name, item.description].some((text) => (text || "").toLowerCase().includes(search))));
  $("catalog-count").textContent = "显示 " + visible.length + " / " + items.length + " 个模型";
  $("catalog-unlisted").hidden = !snapshot.catalog_unlisted_count;
  $("catalog-unlisted").textContent = snapshot.catalog_unlisted_count ? "另有 " + snapshot.catalog_unlisted_count + " 个模型暂未展示。" : "";
  if (!visible.length) { const row = node("tr"), cell = node("td", items.length ? "没有符合条件的模型。" : "暂无可展示的模型目录。", "empty"); cell.colSpan = 7; row.append(cell); $("catalog-rows").append(row); }
  for (const item of visible) {
    const row = node("tr"), model = node("td"), origin = node("td"), state = node("td"), price = node("td"), instruction = node("td"), route = node("td"), operation = node("td");
    model.append(node("div", item.name, "catalog-model-name"), node("div", item.model, "catalog-model-id"));
    if (item.description) model.append(node("div", item.description, "catalog-description"));
    origin.append(node("span", ({ builtin: "内置", custom: "自定义", unknown: "未知" })[item.origin], "table-tag"));
    state.append(status(item.enabled === true ? "enabled" : item.enabled === false ? "disabled" : "unknown"));
    if (item.supported_in_api === false) state.append(node("small", "不支持 API 调用", "catalog-description"));
    const prices = item.price || {};
    price.append(node("span", ({ official: "官方价格", estimated: "估算价格", custom: "自定义价格", missing: "价格未提供", unknown: "价格未知" })[prices.status || "missing"], "table-tag"));
    const values = ["input", "cached_input", "cache_write", "output"].map((key) => prices[key + "_usd_per_million"]);
    price.append(node("div", values.map((value) => value == null ? "—" : new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 6 }).format(value)).join(" / "), "catalog-price"));
    price.title = "USD / 百万 Token：输入 / 缓存读取 / 缓存写入 / 输出";
    instruction.append(node("span", ({ passthrough: "透传", fallback: "兜底", override: "覆盖", unknown: "未知" })[item.instructions_mode || "unknown"], "table-tag"));
    route.append(node("span", item.route_count + " 条路由", "table-tag"));
    if (item.account_pool_routes) route.append(node("div", "账号池 × " + item.account_pool_routes, "route-summary"));
    if (item.aggregate_api_routes) route.append(node("div", "聚合 API × " + item.aggregate_api_routes, "route-summary"));
    if (!item.route_count) route.append(node("div", "暂无启用路由", "route-summary"));
    const readonly = node("span", undefined, "readonly-cell"); readonly.append(icon("lock"), node("span", "仅查看")); operation.append(readonly);
    row.append(model, origin, state, price, instruction, route, operation); $("catalog-rows").append(row);
  }
}
function renderModels() {
  $("models").replaceChildren();
  const modelTotal = snapshot.models_week.reduce((sum, item) => sum + item.usage.total_tokens, 0);
  snapshot.models_week.forEach((item, index) => {
    const tag = node("div", undefined, "model"), heading = node("div", undefined, "model-heading"), label = node("span", undefined, "model-name");
    label.append(node("span", String(index + 1).padStart(2, "0"), "model-rank"), node("span", item.model === "other" ? "其他模型" : item.model));
    heading.append(label, node("span", compact(item.usage.total_tokens) + " Token", "model-value"));
    const share = modelTotal ? item.usage.total_tokens / modelTotal * 100 : 0;
    const track = node("div", undefined, "share"), bar = node("progress"); bar.max = 100; bar.value = share;
    bar.setAttribute("aria-label", item.model + " Token 占比"); track.append(bar, node("span", share.toFixed(1) + "%"));
    tag.append(heading, track); $("models").append(tag);
  });
  if (!snapshot.models_week.length) $("models").append(node("div", "近 7 天暂无模型调用记录。", "empty"));
}
async function refresh() {
  if (fetching) return;
  const generation = sessionVersion; fetching = true; $("refresh").disabled = true;
  try {
    const result = await api("/api/dashboard"); if (generation !== sessionVersion) return;
    snapshot = result.data; $("login-panel").hidden = true; $("dashboard").hidden = false;
    $("updated").textContent = "更新于：" + date(snapshot.generated_at);
    $("sync-badge").textContent = result.stale ? "更新延迟" : "已更新";
    $("sync-badge").className = result.stale ? "badge stale" : "badge";
    $("notice").hidden = !result.stale;
    $("notice").textContent = "更新有点慢，暂时显示上次的数据。";
    renderAccounts(); renderUsage(); renderModels(); renderCatalog(); navigate(view, false);
  } catch (error) {
    if (generation !== sessionVersion) return;
    if (error.status === 401) showLogin("登录已过期，请重新登录。");
    else {
      $("login-panel").hidden = true; $("dashboard").hidden = false; $("notice").hidden = false;
      $("sync-badge").textContent = "更新失败"; $("sync-badge").className = "badge stale";
      $("notice").textContent = "暂时无法更新，请稍后再试。已有数据仍保留上次结果。";
    }
  } finally { fetching = false; $("refresh").disabled = false; }
}
$("login-form").addEventListener("submit", async (event) => {
  event.preventDefault(); $("login-button").disabled = true; $("login-error").textContent = "";
  try {
    const result = await api("/api/login", { method: "POST", headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: $("username").value, password: $("password").value }) });
    csrf = result.csrf; $("password").value = ""; await refresh();
  } catch (error) {
    $("login-error").textContent = error.status === 429 ? "尝试过于频繁，请稍后重试。" : error.status === 401 ? "账号或密码不正确。" : "登录暂不可用，请稍后重试。";
  } finally { $("login-button").disabled = false; }
});
$("logout").addEventListener("click", async () => {
  sessionVersion++;
  try { await api("/api/logout", { method: "POST", headers: { "X-CSRF-Token": csrf } }); showLogin(); }
  catch (error) { if (error.status === 401) showLogin(); else { $("notice").hidden = false; $("notice").textContent = "退出未完成，请重试。"; } }
});
$("refresh").addEventListener("click", refresh);
$("catalog-search").addEventListener("input", () => { if (snapshot) renderCatalog(); });
$("catalog-status").addEventListener("change", () => { if (snapshot) renderCatalog(); });
document.querySelectorAll("[data-period]").forEach((button) => button.addEventListener("click", () => { period = button.dataset.period; if (snapshot) renderUsage(); }));
document.querySelectorAll("[data-view]").forEach((link) => link.addEventListener("click", (event) => {
  if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
  event.preventDefault(); navigate(link.dataset.view);
}));
window.addEventListener("hashchange", () => navigate(location.hash.slice(1), false));
setInterval(() => { if (csrf && !document.hidden) refresh(); }, 60000);
setInterval(() => { if (!document.hidden) for (const item of countdowns) item.element.textContent = countdownText(item.resetsAt); }, 30000);
window.addEventListener("pageshow", async () => {
  navigate(location.hash.slice(1), false);
  try { const result = await api("/api/session"); csrf = result.csrf; await refresh(); } catch { showLogin(); }
});
