"use strict";

const $ = (id) => document.getElementById(id);
const state = {
  csrf: "",
  snapshot: null,
  stale: false,
  view: "overview",
  keyId: null,
  errorFilter: "all",
  keyQuery: "",
  keyStatus: "all",
  modelQuery: "",
  modelStatus: "all",
  loading: false,
  timer: null,
};
const fmt = new Intl.NumberFormat("zh-CN");
const compact = new Intl.NumberFormat("zh-CN", { notation: "compact", maximumFractionDigits: 1 });
const ICONS = {
  grid: ["M4 4h6v6H4z", "M14 4h6v6h-6z", "M4 14h6v6H4z", "M14 14h6v6h-6z"],
  layers: ["m12 3 9 5-9 5-9-5 9-5", "m3 12 9 5 9-5", "m3 16 9 5 9-5"],
  key: ["M15.5 12a5.5 5.5 0 1 0-5.4-4.5L2 16v6h6v-3h3v-3l1.5-1.5", "M16 7h.01"],
  chart: ["M4 19V5", "M4 19h16", "m7 15 3-4 3 2 5-7"],
  alert: ["M12 9v4", "M12 17h.01", "M10.3 3.9 2.6 17a2 2 0 0 0 1.7 3h15.4a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z"],
  zap: ["m13 2-3 12h7l-1 8 8-13h-7z"],
  cube: ["m12 3 9 5v8l-9 5-9-5V8z", "m3 8 9 5 9-5", "M12 13v8"],
  refresh: ["M20 7v5h-5", "M4 17v-5h5", "M5.5 7a8 8 0 0 1 13-2L20 7", "M4 17l1.5 2a8 8 0 0 0 13-2"],
  menu: ["M4 7h16", "M4 12h16", "M4 17h16"],
  close: ["M6 6l12 12", "M18 6 6 18"],
  search: ["M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16z", "m21 21-4.3-4.3"],
};

const PAGE_META = {
  overview: ["CONTROL ROOM", "概览"],
  accounts: ["UPSTREAM POOL", "上游账号"],
  keys: ["MEMBERS / KEYS", "成员与 Key"],
  usage: ["USAGE / ANALYSIS", "用量分析"],
  speed: ["SPEED LAB", "测速"],
  errors: ["ERROR CENTER", "错误中心"],
  models: ["MODEL CATALOG", "模型目录"],
};

function node(tag, text, className) {
  const element = document.createElement(tag);
  if (text !== undefined && text !== null) element.textContent = text;
  if (className) element.className = className;
  return element;
}

function icon(name) {
  const wrapper = node("span", undefined, "nav-icon");
  wrapper.setAttribute("aria-hidden", "true");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  for (const d of ICONS[name] || ICONS.grid) {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", d);
    svg.append(path);
  }
  wrapper.append(svg);
  return wrapper;
}

function mountIcons() {
  document.querySelectorAll("[data-icon]").forEach((element) => {
    const name = element.dataset.icon;
    if (!name) return;
    const svg = icon(name).firstChild;
    element.replaceChildren(svg);
    element.removeAttribute("data-icon");
  });
}

const number = (value) => fmt.format(Number(value) || 0);
const compactNumber = (value) => compact.format(Number(value) || 0);
const money = (cents, digits = 2) => "$" + (Math.max(0, Number(cents) || 0) / 100).toFixed(digits);
const moneyFine = (cents) => money(cents, 4);
const moneySmart = (cents) => money(cents, Number(cents) % 100 === 0 ? 0 : 2);
const percent = (value, digits = 1) => Number.isFinite(Number(value)) ? Number(value).toFixed(digits) + "%" : "—";
const dateTime = (value) => value ? new Date(value * 1000).toLocaleString("zh-CN", {
  timeZone: "Asia/Shanghai", hour12: false, month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
}) : "—";
const dateShort = (value) => value ? new Date(value * 1000).toLocaleString("zh-CN", {
  timeZone: "Asia/Shanghai", hour12: false, month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit",
}) : "—";
const dayLabel = (value) => value ? new Date(value * 1000).toLocaleDateString("zh-CN", {
  timeZone: "Asia/Shanghai", month: "2-digit", day: "2-digit",
}) : "—";
const duration = (value) => !Number.isFinite(Number(value)) ? "—" : Number(value) >= 1000 ? (Number(value) / 1000).toFixed(2) + "s" : Math.round(Number(value)) + "ms";
const timeAgo = (value) => {
  if (!value) return "—";
  const seconds = Math.max(0, Math.floor(Date.now() / 1000 - value));
  if (seconds < 60) return "刚刚";
  if (seconds < 3600) return Math.floor(seconds / 60) + " 分钟前";
  if (seconds < 86400) return Math.floor(seconds / 3600) + " 小时前";
  return Math.floor(seconds / 86400) + " 天前";
};

async function request(path, options = {}) {
  const response = await fetch(path, { cache: "no-store", credentials: "same-origin", ...options });
  if (!response.ok) {
    const error = new Error("request_failed");
    error.status = response.status;
    throw error;
  }
  return response.json();
}

function showLogin(message = "") {
  state.snapshot = null;
  state.csrf = "";
  $("dashboard").hidden = true;
  $("login-panel").hidden = false;
  $("login-error").textContent = message;
  if (state.timer) clearInterval(state.timer);
  state.timer = null;
}

function showDashboard() {
  $("login-panel").hidden = true;
  $("dashboard").hidden = false;
}

async function checkSession() {
  try {
    const session = await request("/api/session");
    state.csrf = session.csrf || "";
    showDashboard();
    await loadSnapshot();
    startRefresh();
  } catch (error) {
    showLogin();
  }
}

async function loadSnapshot({ silent = false } = {}) {
  if (state.loading) return;
  state.loading = true;
  try {
    const payload = await request("/api/dashboard");
    state.snapshot = payload.data;
    state.stale = Boolean(payload.stale);
    const keys = currentKeys();
    if (!state.keyId || !keys.some((key) => key.id === state.keyId)) {
      const remembered = localStorage.getItem("dashboard.key");
      const rememberedKey = keys.find((key) => key.id === remembered);
      const preferred = rememberedKey
        || keys.filter((key) => key.status === "enabled").sort((a, b) => (b.last_used_at || 0) - (a.last_used_at || 0))[0]
        || keys[0];
      state.keyId = preferred ? preferred.id : null;
    }
    renderAll();
    $("notice").hidden = true;
  } catch (error) {
    if (error.status === 401) {
      showLogin();
      return;
    }
    const notice = $("notice");
    notice.textContent = error.status === 503
      ? "暂时无法读取快照：数据正在生成或已过期。页面保留上一次数值。"
      : "数据刷新失败，页面保留上一次数值。";
    notice.hidden = false;
    if (!silent) $("sync-badge").textContent = "刷新失败";
  } finally {
    state.loading = false;
  }
}

function startRefresh() {
  if (state.timer) clearInterval(state.timer);
  state.timer = setInterval(() => {
    if (!document.hidden && state.snapshot) loadSnapshot({ silent: true });
  }, 60_000);
  if (state.liveTimer) clearInterval(state.liveTimer);
  state.liveTimer = setInterval(refreshLive, 3_000);
  refreshLive();
}

async function refreshLive() {
  if (document.hidden || !state.snapshot) return;
  try {
    const payload = await request("/api/live");
    if (!payload.available) return;
    const counts = payload.accounts || {};
    document.querySelectorAll("[data-inflight]").forEach((element) => {
      const count = Number(counts[element.dataset.inflight] || 0);
      element.textContent = `${count} / 5`;
      element.classList.toggle("busy", count >= 5);
    });
    const total = Object.values(counts).reduce((sum, value) => sum + Number(value || 0), 0);
    const capacity = (state.snapshot.accounts || []).filter((account) => account.status === "enabled").length * 5;
    const node = $("pool-capacity");
    if (node) node.textContent = capacity ? `${total} / ${capacity} 路` : "0 路";
  } catch {
    // Keep the last known values; the next poll retries.
  }
}

function currentKeys() {
  return (state.snapshot?.keys || []).filter((key) => key.is_historical !== true);
}

function selectedKey() {
  return currentKeys().find((key) => key.id === state.keyId) || null;
}

function activeKeys() {
  return currentKeys().filter((key) => key.status === "enabled");
}

function healthyAccounts() {
  return (state.snapshot?.accounts || []).filter((account) => account.status === "enabled");
}

function weeklyWindow(account) {
  const primary = account.primary || {};
  const secondary = account.secondary || {};
  if (primary.minutes >= 10080 || primary.minutes > 1440) return primary;
  if (secondary.minutes >= 10080 || secondary.minutes > 1440) return secondary;
  return primary.minutes ? primary : secondary;
}

function accountRemaining(account) {
  const window = weeklyWindow(account);
  return window && Number.isFinite(window.remaining_percent) ? window.remaining_percent : null;
}

function accountStatus(account) {
  if (account.status !== "enabled") return { label: account.status === "disabled" ? "已停用" : "不可用", tone: "bad" };
  const remaining = accountRemaining(account);
  if (remaining === 0) return { label: "额度已耗尽", tone: "bad" };
  if (remaining === null) return { label: "额度未知", tone: "warn" };
  if (remaining < 15) return { label: "容量紧张", tone: "warn" };
  return { label: "可调度", tone: "good" };
}

function personLabel(key) {
  return key.label || "未命名成员";
}

function initials(name) {
  const text = String(name || "").trim();
  if (!text) return "?";
  if (/^[\u4e00-\u9fa5]/.test(text)) return text.slice(-2);
  return text.slice(0, 2).toUpperCase();
}

function setView(next, { hash = true } = {}) {
  const view = PAGE_META[next] ? next : "overview";
  state.view = view;
  document.querySelectorAll(".view").forEach((section) => { section.hidden = section.id !== "view-" + view; });
  document.querySelectorAll(".nav a[data-view]").forEach((link) => {
    if (link.dataset.view === view) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  });
  const [kicker, title] = PAGE_META[view];
  $("page-kicker").textContent = kicker;
  $("page-title").textContent = title;
  $("sidebar").classList.remove("open");
  if (hash && location.hash !== "#" + view) history.replaceState(null, "", "#" + view);
  window.scrollTo({ top: 0, behavior: "smooth" });
}

function statusPill(label, tone = "") {
  return node("span", label, "status-pill " + tone);
}

function statCard(label, value, note) {
  const article = node("article");
  article.append(node("span", label), node("strong", value), node("small", note));
  return article;
}

function empty(text) {
  return node("p", text, "empty");
}

/* ---------- renderers ---------- */

function renderMemberSelect() {
  const select = $("member-select");
  const keys = currentKeys().slice().sort((a, b) => {
    if (a.status !== b.status) return a.status === "enabled" ? -1 : 1;
    return String(a.label).localeCompare(String(b.label), "zh-CN");
  });
  select.replaceChildren();
  if (!keys.length) {
    const option = node("option", "暂无成员");
    option.value = "";
    select.append(option);
    select.disabled = true;
    return;
  }
  select.disabled = false;
  for (const key of keys) {
    const option = node("option", personLabel(key) + (key.status === "enabled" ? "" : " · 停用"));
    option.value = key.id;
    select.append(option);
  }
  if (state.keyId) select.value = state.keyId;
}

function renderMyQuota() {
  const key = selectedKey();
  if (!key) {
    $("my-available").textContent = "—";
    $("my-used").textContent = "—";
    $("my-progress").value = 0;
    $("my-limit").textContent = "上限 $240";
    $("my-release").textContent = "暂无成员 Key";
    $("my-last-use").textContent = "最近使用：—";
    return;
  }
  const limit = Math.max(1, Number(key.weekly_limit_cents) || 24000);
  const used = Math.max(0, Number(key.weekly_used_cents) || 0);
  const remaining = key.weekly_remaining_cents == null ? Math.max(0, limit - used) : key.weekly_remaining_cents;
  $("my-available").textContent = money(remaining, remaining % 100 === 0 ? 0 : 2);
  $("my-used").textContent = money(used, used % 100 === 0 ? 0 : 2) + " / " + money(limit, 0);
  $("my-limit").textContent = "上限 " + money(limit, 0);
  const progress = $("my-progress");
  progress.max = limit;
  progress.value = Math.min(limit, used);
  progress.classList.toggle("low", used / limit >= 0.8);
  progress.classList.toggle("empty", used === 0);
  if (key.next_release_at && key.next_release_cents > 0) {
    $("my-release").textContent = "下一笔 " + money(key.next_release_cents) + " 于 " + dateShort(key.next_release_at) + " 释放";
  } else {
    $("my-release").textContent = "滚动 7 天 · 逐笔释放";
  }
  $("my-last-use").textContent = "最近使用：" + (key.last_used_at ? timeAgo(key.last_used_at) : "暂无记录");
}

function renderMyStatus() {
  const key = selectedKey();
  const enabled = healthyAccounts();
  const capacity = enabled.filter((account) => (accountRemaining(account) ?? 0) > 0).length * 5;
  const remaining = key ? Math.max(0, Number(key.weekly_remaining_cents) || 0) : 0;
  const hasQuota = Boolean(key && remaining > 0 && key.status === "enabled");
  const hasPool = capacity > 0;
  let label = "可以开始任务";
  let tone = "good";
  let note = "个人额度可用，共享池至少有一个账号可调度。";
  if (!key) {
    label = "未选择成员";
    tone = "warn";
    note = "请选择一个成员 Key 查看额度。";
  } else if (key.status !== "enabled") {
    label = "Key 已停用";
    tone = "bad";
    note = "这个 Key 不再接收新请求。";
  } else if (!hasQuota) {
    label = "个人额度已用尽";
    tone = "bad";
    note = "滚动 7 天额度已用完，等待逐笔释放或由管理员调整上限。";
  } else if (!hasPool) {
    label = "共享池暂不可用";
    tone = "bad";
    note = "个人额度还有，但三个上游账号当前都不可调度。";
  } else if (remaining < 24000 * 0.15) {
    label = "额度偏低";
    tone = "warn";
    note = "个人额度低于 15%，建议优先使用轻量模型或等待释放。";
  }
  const chip = $("my-state-chip");
  chip.className = "status-chip " + tone;
  $("my-state").textContent = label;
  $("my-state-note").textContent = note;
  $("pool-capacity").textContent = capacity > 0 ? capacity + " 路 / " + enabled.length + " 账号" : "0 路";
}

function renderPool() {
  const container = $("pool-accounts");
  container.replaceChildren();
  const accounts = (state.snapshot?.accounts || []).slice().sort((a, b) => (a.sort_order || 0) - (b.sort_order || 0));
  if (!accounts.length) {
    container.append(empty("暂无上游账号数据。"));
    return;
  }
  accounts.forEach((account, index) => {
    const card = node("article", undefined, "card pool-card");
    const top = node("div", undefined, "pool-top");
    const name = node("div", undefined, "pool-name");
    const avatar = node("span", String.fromCharCode(65 + index), "pool-avatar");
    const text = node("div");
    text.append(node("strong", "账号 " + String.fromCharCode(65 + index)), node("small", account.label + " · " + (account.plan === "unknown" ? "套餐未知" : account.plan.toUpperCase())));
    name.append(avatar, text);
    const status = accountStatus(account);
    top.append(name, statusPill(status.label, status.tone));
    card.append(top);

    const remaining = accountRemaining(account);
    const percentBox = node("div", undefined, "pool-percent");
    percentBox.append(node("strong", remaining == null ? "—" : percent(remaining)), node("span", "上游周配额剩余"));
    card.append(percentBox);
    const progress = node("progress");
    progress.max = 100;
    progress.value = remaining == null ? 0 : Math.min(100, remaining);
    progress.classList.toggle("low", remaining != null && remaining < 20);
    progress.classList.toggle("empty", remaining == null);
    card.append(progress);

    const window = weeklyWindow(account);
    const meta = node("div", undefined, "pool-meta");
    meta.append(node("span", "重置 " + (window?.resets_at ? dateShort(window.resets_at) : "待更新")));
    meta.append(node("span", "并发上限 "), node("b", "5"));
    const live = node("b", "—");
    live.dataset.inflight = account.id;
    meta.append(node("span", "实时 "), live);
    card.append(meta);

    const ref = node("div", undefined, "pool-ref");
    ref.append(node("span", "站内 API 等值"), node("b", money(account.weekly_used_cents) + " / " + money(account.weekly_limit_cents, 0)));
    card.append(ref);
    card.append(node("p", "采集于 " + dateShort(account.captured_at), "fineprint"));
    container.append(card);
  });
}

function renderStats() {
  const today = state.snapshot?.request_health?.today;
  if (!today) {
    for (const id of ["stat-requests", "stat-success", "stat-failures", "stat-first"]) $(id).textContent = "—";
    return;
  }
  $("stat-requests").textContent = number(today.requests);
  $("stat-success").textContent = percent(today.success_rate);
  $("stat-failures").textContent = number(today.failed);
  $("stat-first").textContent = duration(today.average_first_response_ms);
}

function renderTrend(containerId) {
  const container = $(containerId);
  container.replaceChildren();
  const days = (state.snapshot?.request_health?.days || []).slice(-14);
  if (!days.length) {
    container.append(empty("暂无按日记录。"));
    return;
  }
  const max = Math.max(1, ...days.map((day) => day.requests || 0));
  for (const day of days) {
    const column = node("div", undefined, "trend-day");
    const bar = node("div", undefined, "trend-bar");
    const okHeight = Math.round(((day.success || 0) / max) * 100);
    const badHeight = Math.round(((day.failed || 0) / max) * 100);
    const ok = node("i", undefined, "trend-seg ok");
    ok.style.height = okHeight + "%";
    bar.append(ok);
    if (badHeight > 0) {
      const bad = node("i", undefined, "trend-seg bad");
      bad.style.height = badHeight + "%";
      bar.append(bad);
    }
    column.append(bar, node("span", dayLabel(day.date ? Date.parse(day.date + "T00:00:00+08:00") / 1000 : null), "trend-label"));
    column.title = `${day.date || ""} · 完成 ${day.success || 0} · 失败 ${day.failed || 0}`;
    container.append(column);
  }
}

function errorGroup(failure) {
  const status = Number(failure.status_code) || 0;
  const category = failure.category || "";
  const message = failure.message || "";
  if (status === 499 || category === "client_cancelled") return "client";
  if (/周额度|quota|并发|队列|额度已用尽|no available account/i.test(message)) return "capacity";
  if (status === 429 && !/周额度|quota/i.test(message)) return "upstream";
  if (status >= 500 || ["upstream_overloaded", "upstream_error", "timeout", "network_error"].includes(category)) return "upstream";
  return "request";
}

const GROUP_LABEL = {
  capacity: "策略与容量",
  upstream: "上游与网关",
  request: "配置与请求",
  client: "客户端取消",
};
const CATEGORY_LABEL = {
  no_available_account: "无可用账号",
  rate_limited: "限流",
  auth_error: "鉴权失败",
  timeout: "超时",
  client_cancelled: "客户端取消",
  network_error: "网络中断",
  upstream_overloaded: "上游过载",
  invalid_request: "请求无效",
  upstream_error: "上游错误",
  unknown_error: "未知错误",
};

function renderErrorSummary() {
  const container = $("error-summary");
  container.replaceChildren();
  const failures = state.snapshot?.recent_failures || [];
  if (!failures.length) {
    container.append(empty("当前没有记录到失败请求。"));
    return;
  }
  const groups = new Map();
  for (const failure of failures) {
    const group = errorGroup(failure);
    const item = groups.get(group) || { count: 0, last: failure };
    item.count += 1;
    if ((failure.occurred_at || 0) > (item.last.occurred_at || 0)) item.last = failure;
    groups.set(group, item);
  }
  [...groups.entries()]
    .sort((a, b) => b[1].count - a[1].count)
    .slice(0, 3)
    .forEach(([group, item]) => {
      const row = node("div", undefined, "error-row");
      row.append(node("span", undefined, "error-mark"));
      const main = node("div");
      main.append(node("strong", GROUP_LABEL[group]), node("small", item.last.message || CATEGORY_LABEL[item.last.category] || "失败请求"));
      row.append(main, node("span", item.count + " 次", "error-count"));
      container.append(row);
    });
}

function renderModelMix() {
  const container = $("model-mix");
  container.replaceChildren();
  const models = (state.snapshot?.models_week || [])
    .filter((item) => item.usage && item.usage.estimated_usd > 0)
    .sort((a, b) => b.usage.estimated_usd - a.usage.estimated_usd)
    .slice(0, 6);
  if (!models.length) {
    container.append(empty("近 7 天暂无费用记录。"));
    return;
  }
  const max = Math.max(...models.map((item) => item.usage.estimated_usd));
  for (const item of models) {
    const row = node("div", undefined, "mix-row");
    const name = node("span", item.model, "mix-name");
    const track = node("div", undefined, "mix-track");
    const fill = node("i", undefined, "mix-fill");
    fill.style.width = Math.max(2, (item.usage.estimated_usd / max) * 100) + "%";
    track.append(fill);
    row.append(name, track, node("span", money(item.usage.estimated_usd * 100), "mix-value"));
    container.append(row);
  }
}

function renderAccounts() {
  const container = $("account-cards");
  container.replaceChildren();
  const accounts = (state.snapshot?.accounts || []).slice().sort((a, b) => (a.sort_order || 0) - (b.sort_order || 0));
  if (!accounts.length) {
    container.append(empty("暂无上游账号。"));
    return;
  }
  accounts.forEach((account, index) => {
    const card = node("article", undefined, "card account-card");
    const head = node("div", undefined, "account-card-head");
    const identity = node("div", undefined, "account-id");
    identity.append(node("span", String.fromCharCode(65 + index), "pool-avatar"));
    const title = node("div");
    title.append(node("h3", "账号 " + String.fromCharCode(65 + index)), node("p", account.label + " · " + (account.plan === "unknown" ? "套餐未知" : account.plan.toUpperCase())));
    identity.append(title);
    const status = accountStatus(account);
    head.append(identity, node("span", status.label, "account-status " + status.tone));
    card.append(head);

    const main = node("div", undefined, "account-main");
    const quota = node("div");
    const remaining = accountRemaining(account);
    quota.append(node("p", "上游原生周配额剩余", "account-quota-label"));
    const big = node("div", undefined, "account-quota-large");
    big.append(node("strong", remaining == null ? "—" : percent(remaining)), node("span", "上游口径"));
    quota.append(big);
    const progress = node("progress");
    progress.max = 100;
    progress.value = remaining == null ? 0 : Math.min(100, remaining);
    progress.classList.toggle("low", remaining != null && remaining < 20);
    progress.classList.toggle("empty", remaining == null);
    quota.append(progress);

    const facts = node("dl", undefined, "account-facts");
    const window = weeklyWindow(account);
    const fact = (label, value) => {
      const row = node("div");
      row.append(node("dt", label), node("dd", value));
      facts.append(row);
    };
    fact("重置时间", window?.resets_at ? dateShort(window.resets_at) : "待更新");
    fact("并发上限", "5 路");
    const liveRow = node("div");
    const liveValue = node("dd", "—");
    liveValue.dataset.inflight = account.id;
    liveRow.append(node("dt", "实时并发"), liveValue);
    facts.append(liveRow);
    fact("站内 API 等值", money(account.weekly_used_cents) + " / " + money(account.weekly_limit_cents, 0));
    fact("数据采集", account.captured_at ? dateShort(account.captured_at) : "待更新");
    main.append(quota, facts);
    card.append(main);

    const details = node("details", undefined, "account-details");
    details.append(node("summary", "管理员信息与订阅字段"));
    const grid = node("div", undefined, "detail-grid");
    const detail = (label, value) => {
      const item = node("div");
      item.append(node("span", label), node("strong", value || "—"));
      grid.append(item);
    };
    detail("分组", account.group_name || "未分组");
    detail("订阅续期", account.subscription_renews_at ? dateShort(account.subscription_renews_at) : "—");
    detail("订阅到期", account.subscription_expires_at ? dateShort(account.subscription_expires_at) : "—");
    detail("代理", { configured: "已配置", inherited: "继承", disabled: "停用", unknown: "未知" }[account.proxy_state] || "未知");
    detail("排序", account.sort_order == null ? "—" : String(account.sort_order));
    detail("容量覆盖", account.capacity_override ? "是" : "否");
    details.append(grid);
    card.append(details);
    container.append(card);
  });
}

function filteredKeys() {
  const query = state.keyQuery.trim().toLowerCase();
  return currentKeys()
    .filter((key) => state.keyStatus === "all" || key.status === state.keyStatus)
    .filter((key) => !query || personLabel(key).toLowerCase().includes(query) || key.id.toLowerCase().includes(query))
    .sort((a, b) => {
      if (a.status !== b.status) return a.status === "enabled" ? -1 : 1;
      return (Number(b.weekly_used_cents) || 0) - (Number(a.weekly_used_cents) || 0) || personLabel(a).localeCompare(personLabel(b), "zh-CN");
    });
}

function renderKeys() {
  const body = $("keys-table");
  body.replaceChildren();
  const keys = filteredKeys();
  const active = currentKeys().filter((key) => key.status === "enabled").length;
  const disabled = currentKeys().length - active;
  $("key-count-note").textContent = `${active} 个启用 · ${disabled} 个停用`;
  $("nav-key-count").textContent = String(active);
  if (!keys.length) {
    const row = node("tr");
    const cell = node("td", "没有符合条件的成员。", "empty");
    cell.colSpan = 5;
    row.append(cell);
    body.append(row);
    return;
  }
  for (const key of keys) {
    const limit = Math.max(1, Number(key.weekly_limit_cents) || 24000);
    const used = Math.max(0, Number(key.weekly_used_cents) || 0);
    const remaining = key.weekly_remaining_cents == null ? Math.max(0, limit - used) : key.weekly_remaining_cents;
    const row = node("tr");
    row.tabIndex = 0;
    row.setAttribute("role", "button");
    row.setAttribute("aria-label", "查看 " + personLabel(key) + " 的 Key 详情");

    const member = node("td");
    const memberCell = node("div", undefined, "member-cell");
    memberCell.append(node("span", initials(personLabel(key)), "member-avatar"));
    const names = node("div");
    names.append(node("strong", personLabel(key)), node("small", key.group_name || "未分组"));
    memberCell.append(names);
    member.append(memberCell);

    const available = node("td");
    available.append(node("span", moneySmart(remaining), "money"));

    const quota = node("td");
    const mini = node("div", undefined, "quota-mini");
    const progress = node("progress");
    progress.max = limit;
    progress.value = Math.min(limit, used);
    progress.classList.toggle("low", used / limit >= 0.8);
    mini.append(progress, node("small", moneySmart(used) + " / " + money(limit, 0)));
    quota.append(mini);

    const status = node("td");
    status.append(statusPill(key.status === "enabled" ? "启用" : "停用", key.status === "enabled" ? "good" : "bad"));

    const last = node("td", key.last_used_at ? timeAgo(key.last_used_at) : "—", "time-cell");
    row.append(member, available, quota, status, last);
    row.addEventListener("click", () => openKeyDrawer(key));
    row.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") { event.preventDefault(); openKeyDrawer(key); }
    });
    body.append(row);
  }
}

function drawerSection(title) {
  const section = node("section", undefined, "drawer-section");
  section.append(node("h3", title));
  return section;
}

function openKeyDrawer(key) {
  const limit = Math.max(1, Number(key.weekly_limit_cents) || 24000);
  const used = Math.max(0, Number(key.weekly_used_cents) || 0);
  const remaining = key.weekly_remaining_cents == null ? Math.max(0, limit - used) : key.weekly_remaining_cents;
  const body = $("drawer-body");
  body.replaceChildren();

  const amountSection = drawerSection("当前可用额度");
  amountSection.append(node("div", moneySmart(remaining), "drawer-amount"));
  const progress = node("progress");
  progress.max = limit;
  progress.value = Math.min(limit, used);
  amountSection.append(progress, node("p", "近 7 天已用 " + moneySmart(used) + " / " + money(limit, 0) + " · 滚动窗口", "drawer-note"));
  body.append(amountSection);

  const metaSection = drawerSection("Key 元数据");
  const meta = node("dl", undefined, "drawer-kv");
  const kv = (label, value) => {
    const row = node("div");
    row.append(node("dt", label), node("dd", value || "—"));
    meta.append(row);
  };
  kv("状态", key.status === "enabled" ? "启用" : "停用");
  kv("分组", key.group_name || "未分组");
  kv("协议", key.protocol || "未知");
  kv("轮转", key.rotation || "未知");
  kv("绑定模型", key.bound_model || "随请求");
  kv("显示编号", key.display_id || "—");
  kv("最近使用", key.last_used_at ? dateTime(key.last_used_at) : "—");
  metaSection.append(meta);
  body.append(metaSection);

  const usageSection = drawerSection("用量");
  const usageMeta = node("dl", undefined, "drawer-kv");
  const addUsage = (label, usage) => {
    if (!usage) return;
    const row = node("div");
    row.append(node("dt", label), node("dd", number(usage.requests) + " 次 · " + moneySmart(usage.estimated_usd * 100)));
    usageMeta.append(row);
  };
  addUsage("今日", key.usage?.today);
  addUsage("近 7 天", key.usage?.week);
  addUsage("累计", key.usage?.recorded);
  usageSection.append(usageMeta);
  body.append(usageSection);

  const releaseSection = drawerSection("额度释放");
  const release = node("dl", undefined, "drawer-kv");
  const releaseRow = node("div");
  releaseRow.append(node("dt", "下一笔释放"), node("dd", key.next_release_at && key.next_release_cents > 0
    ? money(key.next_release_cents) + " · " + dateTime(key.next_release_at)
    : "暂无到期的滚动消费"));
  release.append(releaseRow);
  releaseSection.append(release, node("p", "滚动 7 天不是周一统一清零；每小时消费在满 168 小时后逐笔退出窗口。", "drawer-note"));
  body.append(releaseSection);

  $("drawer-title").textContent = personLabel(key);
  $("drawer-root").hidden = false;
  $("drawer-close").focus();
}

function closeDrawer() {
  $("drawer-root").hidden = true;
}

function renderUsage() {
  const totals = state.snapshot?.totals;
  const container = $("usage-totals");
  container.replaceChildren();
  const week = totals?.week;
  if (!week) {
    container.append(empty("暂无用量数据。"));
  } else {
    container.append(
      statCard("今日请求", number(totals?.today?.requests), "HTTP 请求"),
      statCard("近 7 天参考金额", money(week.estimated_usd * 100), "API 等值"),
      statCard("近 7 天总 Token", compactNumber(week.total_tokens), "输入 + 输出"),
      statCard("缓存输入占比", percent(week.input_tokens ? (week.cached_tokens / week.input_tokens) * 100 : null), "缓存 / 输入"),
    );
  }
  renderUsageChart();
  renderUsageModels();
  renderUsageMembers();
}

function renderUsageChart() {
  const container = $("usage-chart");
  container.replaceChildren();
  const days = (state.snapshot?.request_health?.days || []).slice(-14);
  if (!days.length) {
    container.append(empty("暂无按日记录。"));
    return;
  }
  const max = Math.max(1, ...days.map((day) => day.requests || 0));
  for (const day of days) {
    const column = node("div", undefined, "usage-day");
    const bar = node("div", undefined, "usage-bar");
    const fill = node("i");
    fill.style.height = Math.max(2, ((day.requests || 0) / max) * 100) + "%";
    bar.append(fill);
    column.append(bar, node("span", dayLabel(day.date ? Date.parse(day.date + "T00:00:00+08:00") / 1000 : null), "usage-label"));
    column.title = `${day.date || ""} · ${day.requests || 0} 次`;
    container.append(column);
  }
}

function renderUsageModels() {
  const container = $("usage-models");
  container.replaceChildren();
  const models = (state.snapshot?.models_week || [])
    .filter((item) => item.usage && item.usage.estimated_usd > 0)
    .sort((a, b) => b.usage.estimated_usd - a.usage.estimated_usd)
    .slice(0, 8);
  if (!models.length) {
    container.append(empty("近 7 天暂无费用记录。"));
    return;
  }
  const max = Math.max(...models.map((item) => item.usage.estimated_usd));
  for (const item of models) {
    const row = node("div", undefined, "usage-model-row");
    row.append(node("span", item.model, "usage-model-name"));
    const track = node("div", undefined, "usage-model-track");
    const fill = node("i");
    fill.style.width = Math.max(2, (item.usage.estimated_usd / max) * 100) + "%";
    track.append(fill);
    row.append(track, node("span", money(item.usage.estimated_usd * 100), "usage-model-value"));
    container.append(row);
  }
}

function renderUsageMembers() {
  const body = $("usage-members");
  body.replaceChildren();
  const keys = currentKeys()
    .filter((key) => key.usage?.week)
    .slice()
    .sort((a, b) => (b.usage.week.estimated_usd || 0) - (a.usage.week.estimated_usd || 0));
  const total = keys.reduce((sum, key) => sum + (key.usage.week.estimated_usd || 0), 0);
  if (!keys.length) {
    const row = node("tr");
    const cell = node("td", "近 7 天暂无成员用量。", "empty");
    cell.colSpan = 6;
    row.append(cell);
    body.append(row);
    return;
  }
  for (const key of keys) {
    const week = key.usage.week;
    const share = total > 0 ? (week.estimated_usd || 0) / total : 0;
    const row = node("tr");

    const member = node("td");
    const memberCell = node("div", undefined, "member-cell");
    memberCell.append(node("span", initials(personLabel(key)), "member-avatar"));
    const names = node("div");
    names.append(node("strong", personLabel(key)), node("small", key.group_name || "未分组"));
    memberCell.append(names);
    member.append(memberCell);

    row.append(member);
    row.append(node("td", number(week.requests || 0), "num"));
    row.append(node("td", moneySmart((week.estimated_usd || 0) * 100), "num"));

    const shareCell = node("td");
    const track = node("div", undefined, "share-track");
    const fill = node("i");
    fill.style.width = Math.max(2, share * 100) + "%";
    track.append(fill);
    shareCell.append(track, node("small", percent(share * 100), "share-note"));
    row.append(shareCell);

    const limit = Math.max(1, Number(key.weekly_limit_cents) || 24000);
    const used = Math.max(0, Number(key.weekly_used_cents) || 0);
    const remaining = key.weekly_remaining_cents == null ? Math.max(0, limit - used) : key.weekly_remaining_cents;
    row.append(node("td", key.status === "enabled" ? moneySmart(remaining) : "—", "num"));
    const status = node("td");
    status.append(statusPill(key.status === "enabled" ? "启用" : "停用", key.status === "enabled" ? "good" : "bad"));
    row.append(status);
    body.append(row);
  }
}

function filteredFailures() {
  const query = $("error-search").value.trim().toLowerCase();
  return (state.snapshot?.recent_failures || []).filter((failure) => {
    if (state.errorFilter !== "all" && errorGroup(failure) !== state.errorFilter) return false;
    if (!query) return true;
    return [failure.model, failure.key_label, failure.account_label, failure.message, failure.category]
      .filter(Boolean).join(" ").toLowerCase().includes(query);
  });
}

function renderErrors() {
  const all = state.snapshot?.recent_failures || [];
  const strip = $("error-summary-strip");
  strip.replaceChildren();
  const count = (group) => all.filter((failure) => errorGroup(failure) === group).length;
  strip.append(
    statCard("失败请求", number(all.length), "最近记录"),
    statCard("策略与容量", number(count("capacity")), "额度 / 并发 / 队列"),
    statCard("上游与网关", number(count("upstream")), "过载 / 5xx / 超时"),
    statCard("客户端取消", number(count("client")), "主动停止 / 断开"),
  );

  const container = $("error-list");
  container.replaceChildren();
  const failures = filteredFailures();
  if (!failures.length) {
    container.append(empty("没有符合筛选条件的错误。"));
    return;
  }
  for (const failure of failures) {
    const group = errorGroup(failure);
    const card = node("article", undefined, "error-card");
    const code = Number(failure.status_code) || 0;
    card.append(node("div", String(code), "error-code " + group));
    const main = node("div", undefined, "error-main");
    const categoryLabel = CATEGORY_LABEL[failure.category] || "请求失败";
    main.append(node("strong", categoryLabel === GROUP_LABEL[group] ? categoryLabel : GROUP_LABEL[group] + " · " + categoryLabel));
    main.append(node("p", failure.message || "没有更多错误信息。"));
    const meta = node("div", undefined, "error-meta");
    for (const item of [
      failure.model ? "模型 " + failure.model : "",
      failure.key_label ? "成员 " + failure.key_label : "",
      failure.account_label ? "账号 " + failure.account_label : "",
      failure.duration_ms != null ? "耗时 " + duration(failure.duration_ms) : "",
    ].filter(Boolean)) meta.append(node("span", item));
    main.append(meta);
    card.append(main, node("div", failure.occurred_at ? dateTime(failure.occurred_at) : "—", "error-time"));
    container.append(card);
  }
}

function price(value) {
  return Number.isFinite(Number(value)) ? "$" + Number(value).toFixed(2) : "—";
}

function renderModels() {
  const body = $("model-table");
  body.replaceChildren();
  const query = state.modelQuery.trim().toLowerCase();
  const models = (state.snapshot?.model_catalog || [])
    .filter((model) => !String(model.model || "").toLowerCase().startsWith("claude"))
    .filter((model) => state.modelStatus === "all" || (state.modelStatus === "enabled" ? model.enabled !== false : model.enabled === false))
    .filter((model) => !query || model.model.toLowerCase().includes(query) || (model.name || "").toLowerCase().includes(query))
    .sort((a, b) => Number(b.enabled !== false) - Number(a.enabled !== false) || a.model.localeCompare(b.model));
  if (!models.length) {
    const row = node("tr");
    const cell = node("td", "没有符合条件的模型。", "empty");
    cell.colSpan = 6;
    row.append(cell);
    body.append(row);
    return;
  }
  for (const model of models) {
    const row = node("tr");
    const name = node("td");
    name.append(node("strong", model.model, "money"));
    if (model.name && model.name !== model.model) name.append(node("small", model.name, "model-sub"));
    if (model.description) name.append(node("div", model.description, "fineprint"));
    row.append(name);
    row.append(node("td", model.enabled === false ? "已禁用" : "已启用", "status-pill " + (model.enabled === false ? "bad" : "good")));
    row.append(node("td", price(model.price?.input_usd_per_million), "money"));
    row.append(node("td", price(model.price?.cached_input_usd_per_million), "money"));
    row.append(node("td", price(model.price?.output_usd_per_million), "money"));
    row.append(node("td", number(model.route_count || 0), "money"));
    body.append(row);
  }
}

function tps(value) {
  return Number.isFinite(Number(value)) ? Number(value).toFixed(1) : "—";
}

function renderSpeed() {
  const speed = state.snapshot?.speed;
  const decode = speed?.decode_p50;
  $("speed-decode-p50").textContent = tps(decode);
  const facts = $("speed-facts");
  facts.replaceChildren();
  for (const [label, value] of [
    ["端到端 p50", speed?.e2e_p50 == null ? "—" : tps(speed.e2e_p50) + " tok/s"],
    ["解码 p90", speed?.decode_p90 == null ? "—" : tps(speed.decode_p90) + " tok/s"],
    ["端到端 p90", speed?.e2e_p90 == null ? "—" : tps(speed.e2e_p90) + " tok/s"],
    ["输出 Token", speed ? number(speed.output_tokens) : "—"],
    ["有效样本", speed ? number(speed.samples) : "—"],
  ]) {
    const item = document.createElement("div");
    item.append(node("dt", label), node("dd", value));
    facts.append(item);
  }

  const body = $("speed-model-body");
  body.replaceChildren();
  const rows = (speed?.models || []).slice().sort((a, b) => (b.samples || 0) - (a.samples || 0));
  if (!rows.length) {
    const cell = node("td", "近 7 天还没有足够的成功请求。", "empty");
    cell.colSpan = 5;
    const row = document.createElement("tr");
    row.append(cell);
    body.append(row);
  } else {
    for (const item of rows) {
      const row = document.createElement("tr");
      row.append(node("td", item.model, "mono"));
      row.append(node("td", number(item.samples), "num"));
      row.append(node("td", number(item.output_tokens), "num"));
      row.append(node("td", item.decode_p50 == null ? "—" : tps(item.decode_p50) + " tok/s", "num"));
      row.append(node("td", item.e2e_p50 == null ? "—" : tps(item.e2e_p50) + " tok/s", "num"));
      body.append(row);
    }
  }

  const select = $("speed-model");
  const previous = select.value;
  select.replaceChildren();
  const models = (state.snapshot?.model_catalog || [])
    .filter((model) => model.enabled !== false)
    .filter((model) => !String(model.model || "").toLowerCase().startsWith("claude"))
    .filter((model) => !String(model.model || "").toLowerCase().includes("auto-review"))
    .map((model) => model.model);
  const preferred = models.includes("gpt-6-astra") ? "gpt-6-astra" : models[0];
  for (const model of models) {
    const option = node("option", model);
    option.value = model;
    select.append(option);
  }
  if (models.includes(previous)) select.value = previous;
  else if (preferred) select.value = preferred;
  $("speed-updated").textContent = "快照 " + (state.snapshot?.generated_at ? dateTime(state.snapshot.generated_at) : "—");
}

function renderSpeedResult(result) {
  $("speed-result").hidden = false;
  $("speed-result-tps").textContent = tps(result.decode_tps);
  const facts = $("speed-result-facts");
  facts.replaceChildren();
  for (const [label, value] of [
    ["首字", duration(result.first_delta_ms)],
    ["总耗时", duration(result.duration_ms)],
    ["输出 Token", number(result.output_tokens)],
    ["端到端", tps(result.e2e_tps) + " tok/s"],
  ]) {
    const item = document.createElement("div");
    item.append(node("dt", label), node("dd", value));
    facts.append(item);
  }
  $("speed-result-text").textContent = result.text || "";
}

function renderAll() {
  if (!state.snapshot) return;
  renderMemberSelect();
  renderMyQuota();
  renderMyStatus();
  renderPool();
  renderStats();
  renderTrend("trend-chart");
  renderErrorSummary();
  renderModelMix();
  renderAccounts();
  renderKeys();
  renderUsage();
  renderSpeed();
  renderErrors();
  renderModels();

  const age = state.snapshot.generated_at ? Math.max(0, Math.floor(Date.now() / 1000 - state.snapshot.generated_at)) : null;
  const stamp = state.snapshot.generated_at ? dateTime(state.snapshot.generated_at) : "—";
  const badge = $("sync-badge");
  badge.className = "sync-badge" + (state.stale ? " stale" : "");
  badge.replaceChildren(node("span", undefined, "live-dot"), node("span", state.stale ? "数据可能过期" : "数据已同步"));
  $("updated").textContent = `更新于 ${stamp}${age != null ? " · " + age + " 秒前" : ""}`;
  $("accounts-updated").textContent = "采集于 " + (state.snapshot.accounts?.[0]?.captured_at ? dateTime(state.snapshot.accounts[0].captured_at) : "—");
  $("usage-updated").textContent = "快照 " + stamp;
  $("errors-updated").textContent = "最近 " + number((state.snapshot.recent_failures || []).length) + " 条";
  $("models-updated").textContent = number((state.snapshot.model_catalog || []).filter((model) => !String(model.model || "").toLowerCase().startsWith("claude")).length) + " 个模型";
  if (state.stale) {
    const notice = $("notice");
    notice.textContent = "快照超过刷新阈值，页面保留最后一次可用数据；请点右上角刷新。";
    notice.hidden = false;
  }
}

/* ---------- events ---------- */

function bindEvents() {
  document.querySelectorAll(".nav a[data-view]").forEach((link) => {
    link.addEventListener("click", (event) => {
      event.preventDefault();
      setView(link.dataset.view);
    });
  });
  document.querySelectorAll("[data-view]:not(.nav a)").forEach((link) => {
    link.addEventListener("click", (event) => {
      event.preventDefault();
      setView(link.dataset.view);
    });
  });
  $("menu").addEventListener("click", () => $("sidebar").classList.toggle("open"));
  $("refresh").addEventListener("click", () => loadSnapshot());
  $("logout").addEventListener("click", async () => {
    try {
      await request("/api/logout", {
        method: "POST",
        headers: { "Content-Type": "application/json", "X-CSRF-Token": state.csrf },
        body: "{}",
      });
    } catch (error) {
      // The local session may already be gone; always return to login.
    }
    showLogin();
  });
  $("login-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const button = $("login-button");
    $("login-error").textContent = "";
    button.disabled = true;
    button.textContent = "正在验证…";
    try {
      const session = await request("/api/login", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ username: $("username").value, password: $("password").value }),
      });
      state.csrf = session.csrf || "";
      $("password").value = "";
      showDashboard();
      await loadSnapshot();
      startRefresh();
    } catch (error) {
      $("login-error").textContent = error.status === 401
        ? "账号或密码不正确。"
        : error.status === 429 ? "登录尝试过于频繁，请稍后再试。" : "登录失败，请稍后重试。";
    } finally {
      button.disabled = false;
      button.textContent = "进入看板";
    }
  });
  $("member-select").addEventListener("change", (event) => {
    state.keyId = event.target.value || null;
    if (state.keyId) localStorage.setItem("dashboard.key", state.keyId);
    renderMyQuota();
    renderMyStatus();
  });
  $("key-search").addEventListener("input", (event) => { state.keyQuery = event.target.value; renderKeys(); });
  $("key-status-filter").addEventListener("change", (event) => { state.keyStatus = event.target.value; renderKeys(); });
  $("error-search").addEventListener("input", renderErrors);
  $("error-filters").addEventListener("click", (event) => {
    const button = event.target.closest("[data-error-filter]");
    if (!button) return;
    state.errorFilter = button.dataset.errorFilter;
    $("error-filters").querySelectorAll("button").forEach((item) => item.setAttribute("aria-pressed", String(item === button)));
    renderErrors();
  });
  $("speed-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const keyInput = $("speed-key");
    const apiKey = keyInput.value.trim();
    const model = $("speed-model").value;
    const prompt = $("speed-prompt").value.trim();
    const note = $("speed-note");
    if (!apiKey) {
      note.textContent = "请先填入平台 Key；它只用于这一次请求。";
      keyInput.focus();
      return;
    }
    if (!model || !prompt) {
      note.textContent = "请选择模型并填写 Prompt。";
      return;
    }
    const button = $("speed-run");
    button.disabled = true;
    button.textContent = "正在测速…";
    note.textContent = "正在发送一次真实请求，通常需要几秒到几十秒。";
    try {
      const response = await fetch("/api/speedtest", {
        method: "POST",
        credentials: "same-origin",
        cache: "no-store",
        headers: { "Content-Type": "application/json", "X-CSRF-Token": state.csrf },
        body: JSON.stringify({ api_key: apiKey, model, prompt }),
      });
      const payload = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(payload.detail || ("HTTP " + response.status));
      renderSpeedResult(payload);
      note.textContent = "完成。Key 只用于本次请求，刷新页面即清除。";
      $("speed-result").scrollIntoView({ behavior: "auto", block: "nearest" });
    } catch (error) {
      note.textContent = "测速失败：" + (error.message || "请稍后再试");
    } finally {
      button.disabled = false;
      button.textContent = "开始测速";
    }
  });
  $("model-search").addEventListener("input", (event) => { state.modelQuery = event.target.value; renderModels(); });
  $("model-status-filter").addEventListener("change", (event) => { state.modelStatus = event.target.value; renderModels(); });
  $("drawer-close").addEventListener("click", closeDrawer);
  $("drawer-backdrop").addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      closeDrawer();
      $("sidebar").classList.remove("open");
    }
  });
  window.addEventListener("hashchange", () => {
    const next = location.hash.replace("#", "");
    if (PAGE_META[next]) setView(next, { hash: false });
  });
}

async function boot() {
  mountIcons();
  bindEvents();
  const initial = location.hash.replace("#", "");
  setView(PAGE_META[initial] ? initial : "overview", { hash: false });
  await checkSession();
}

boot();
