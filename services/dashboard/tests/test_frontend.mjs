// Pure JavaScript unit tests; no browser, network, credentials, or real account data.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

const script = readFileSync(new URL("../dashboard/static/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../dashboard/static/index.html", import.meta.url), "utf8");
const css = readFileSync(new URL("../dashboard/static/app.css", import.meta.url), "utf8");

class Element {
  constructor(tag = "div") {
    this.tag = tag;
    this.children = [];
    this.attrs = {};
    this.dataset = {};
    this.listeners = {};
    this.hidden = false;
    this.disabled = false;
    this.value = "";
    this.ownText = "";
  }
  append(...items) {
    for (const item of items) {
      if (item !== null && item !== undefined && item !== false) this.children.push(item);
    }
  }
  replaceChildren(...items) {
    this.children = [];
    this.append(...items);
  }
  get firstChild() {
    return this.children[0];
  }
  get childNodes() {
    return this.children;
  }
  get textContent() {
    return (
      this.ownText +
      this.children
        .map((e) => (typeof e === "string" ? e : e.textContent))
        .join("")
    );
  }
  set textContent(value) {
    this.ownText = String(value);
    this.children = [];
  }
  set innerHTML(_) {
    throw new Error("Untrusted HTML insertion is not allowed");
  }
  setAttribute(name, value) {
    assert.notEqual(name.toLowerCase(), "style", "Inline style attribute violates CSP");
    this.attrs[name] = String(value);
  }
  removeAttribute(name) {
    delete this.attrs[name];
  }
  addEventListener(name, callback) {
    this.listeners[name] = callback;
  }
}

function setup(fetchImpl = () => { throw new Error("Unexpected network request"); }, store = new Map()) {
  const elements = new Map([...html.matchAll(/id="([^"]+)"/g)].map((m) => [m[1], new Element()]));
  const nav = ["overview", "errors", "keys", "accounts", "usage", "models", "speed"].map((view) =>
    Object.assign(new Element("button"), { dataset: { view } })
  );
  const periods = ["today", "week", "recorded"].map((period) =>
    Object.assign(new Element("button"), { dataset: { period } })
  );
  const document = {
    hidden: false,
    getElementById: (id) => {
      assert.ok(elements.has(id), "Missing HTML element: " + id);
      return elements.get(id);
    },
    createElement: (tag) => new Element(tag),
    createElementNS: (_, tag) => new Element(tag),
    querySelectorAll: (q) =>
      q === "[data-icon]" ? [] : q.includes("data-view") ? nav : q === "[data-period]" ? periods : [],
  };
  const location = { hash: "" };
  const localStorage = {
    getItem: (k) => store.get(k) ?? null,
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: (k) => store.delete(k),
  };
  const context = vm.createContext({
    __DASHBOARD_TEST__: true,
    document,
    location,
    localStorage,
    history: { replaceState: (_, __, hash) => { location.hash = hash; } },
    window: { addEventListener() {} },
    setInterval() { return 1; },
    clearInterval() {},
    fetch: fetchImpl,
    Intl,
    Date,
    Math,
    Number,
    Error,
  });
  vm.runInContext(script, context);
  return { context, elements, nav, periods, location, store };
}

const usage = (tokens) => ({
  requests: 4,
  input_tokens: tokens,
  cached_tokens: 20,
  output_tokens: 30,
  total_tokens: tokens,
  estimated_usd: 1.25,
});
const blankUsage = () => ({ requests: 0, input_tokens: 0, cached_tokens: 0, output_tokens: 0, total_tokens: 0, estimated_usd: 0 });
const makeHealth = (date, requests, success, sysFail, cancelled, sigs = []) => ({
  date,
  requests,
  success,
  failed: sysFail + cancelled,
  success_rate: requests ? Number(((success * 100) / requests).toFixed(2)) : 0,
  effective_success_rate: requests - cancelled > 0 ? Number(((success * 100) / (requests - cancelled)).toFixed(2)) : 100,
  client_cancelled_count: cancelled,
  system_failed_count: sysFail,
  error_categories: [{ kind: "no_available_account", count: sysFail }, { kind: "client_cancelled", count: cancelled }],
  error_layers: [
    { layer: "gateway_pool", count: sysFail },
    { layer: "upstream_openai", count: 0 },
    { layer: "member_policy", count: 0 },
    { layer: "client_cancelled", count: cancelled },
  ],
  signatures: sigs,
  failure_statuses: sysFail ? [{ status_code: 503, count: sysFail }] : [],
  average_duration_ms: 1400,
  average_first_response_ms: 420,
  last_success_at: 1794650000,
  last_failure_at: sysFail ? 1794649000 : null,
});

const fixture = {
  schema_version: 1,
  generated_at: Math.floor(Date.now() / 1000),
  timezone: "Asia/Shanghai",
  accounts: [
    {
      id: "acct-aaaaaaaaaaaa",
      label: "账号 01",
      group_name: "HPC+AI（大一统）",
      plan: "pro",
      status: "enabled",
      primary: { minutes: 10080, remaining_percent: 62.0, resets_at: Math.floor(Date.now() / 1000) + 7200 },
      secondary: { minutes: null, remaining_percent: null, resets_at: null },
      captured_at: Math.floor(Date.now() / 1000),
      weekly_used_cents: 63719,
      today_requests: 128,
      today_success_rate: 99.2,
    },
  ],
  claude_accounts: [],
  keys: [
    {
      id: "key-bbbbbbbbbbbb",
      label: "张测试",
      group_name: "HPC+AI（大一统）",
      status: "enabled",
      last_used_at: Math.floor(Date.now() / 1000) - 120,
      usage: { today: usage(1000), week: usage(5000), recorded: usage(9000) },
      weekly_used_cents: 12460,
      weekly_limit_cents: 24000,
      weekly_remaining_cents: 11540,
      next_release_at: Math.floor(Date.now() / 1000) + 3600,
      next_release_cents: 25,
    },
  ],
  key_groups: [
    { id: "group-aaaaaaaaaaaa", label: "HPC+AI（大一统）", kind: "named", key_count: 22, usage: { today: usage(1000), week: usage(5000), recorded: usage(9000) } },
    { id: "group-bbbbbbbbbbbb", label: "零用量组", kind: "named", key_count: 1, usage: { today: blankUsage(), week: blankUsage(), recorded: blankUsage() } },
  ],
  totals: { today: usage(1000), week: usage(5000), recorded: usage(9000) },
  pool_usage: {
    openai: { today: usage(1000), week: usage(5000), recorded: usage(9000) },
    claude: { today: blankUsage(), week: blankUsage(), recorded: blankUsage() },
    unattributed: { today: blankUsage(), week: blankUsage(), recorded: blankUsage() },
  },
  models_week: [{ model: "gpt-6-astra", usage: usage(5000) }],
  model_catalog: [
    {
      model: "gpt-6-astra",
      name: "GPT-6-Astra",
      description: '<img src=x onerror="attack()">',
      origin: "builtin",
      enabled: true,
      supported_in_api: true,
      instructions_mode: "passthrough",
      price: { status: "official", input_usd_per_million: 10, cached_input_usd_per_million: 1, cache_write_usd_per_million: null, output_usd_per_million: 50 },
      route_count: 3,
      account_pool_routes: 3,
      aggregate_api_routes: 0,
    },
    {
      model: "gpt-5.6-sol",
      name: "GPT-5.6-Sol",
      description: "Fast workhorse",
      origin: "builtin",
      enabled: false,
      price: { status: "missing", input_usd_per_million: null, cached_input_usd_per_million: null, output_usd_per_million: null },
      route_count: 0,
    },
  ],
  model_weather: [
    { model: "gpt-6-astra", today_requests: 300, week_requests: 1200, today_success_rate: 99.7, week_effective_success_rate: 99.5, today_overload_count: 0, week_overload_count: 2, decode_p50: 20.5, ttft_p50_ms: 3400, status: "smooth" },
  ],
  request_health: {
    today: makeHealth("2026-11-14", 200, 196, 2, 2, [
      {
        signature: "pool_exhausted",
        layer: "gateway_pool",
        severity: "warn",
        title: "共享号池并发打满或无可用候选 (503)",
        meaning: "3 个共享账号当前并发槽位已满。",
        action: "等待 3~10 秒后重试。",
        count: 2,
        status_codes: [503],
        top_models: ["gpt-6-astra"],
        affected_keys: 1,
        last_seen_at: 1794649000,
        sample_message: "HTTP 503 · 共享号池无空闲可用账号",
      },
    ]),
    week: makeHealth("2026-11-14", 500, 490, 5, 5),
    recorded: makeHealth("2026-11-14", 1000, 980, 10, 10),
    days: [
      makeHealth("2026-11-13", 300, 294, 3, 3),
      makeHealth("2026-11-14", 200, 196, 2, 2, [
        {
          signature: "pool_exhausted",
          layer: "gateway_pool",
          severity: "warn",
          title: "共享号池并发打满或无可用候选 (503)",
          meaning: "3 个共享账号当前并发槽位已满。",
          action: "等待 3~10 秒后重试。",
          count: 2,
          status_codes: [503],
          top_models: ["gpt-6-astra"],
          affected_keys: 1,
          last_seen_at: 1794649000,
          sample_message: "HTTP 503 · 共享号池无空闲可用账号",
        },
      ]),
    ],
    history_days: 14,
  },
  recent_failures: [
    {
      occurred_at: 1794649000,
      date: "2026-11-14",
      status_code: 503,
      category: "no_available_account",
      signature: "pool_exhausted",
      layer: "gateway_pool",
      severity: "warn",
      title: "共享号池并发打满或无可用候选 (503)",
      meaning: "3 个共享账号当前并发槽位已满。",
      action: "等待 3~10 秒后重试。",
      model: "gpt-6-astra",
      key_id: "key-bbbbbbbbbbbb",
      key_label: "张测试",
      account_label: "账号 01",
      duration_ms: 820,
      message: "HTTP 503 · 共享号池无空闲可用账号",
    },
  ],
  speed: { samples: 120, output_tokens: 90000, e2e_p50: 15.2, e2e_p90: 26.1, decode_p50: 20.5, decode_p90: 31.4, models: [{ model: "gpt-6-astra", samples: 120, output_tokens: 90000, e2e_p50: 15.2, e2e_p90: 26.1, decode_p50: 20.5, decode_p90: 31.4 }] },
};

test("strict CSP invariants: no inline style, no innerHTML, no external CDN URLs", () => {
  assert.ok(!/style\s*=/.test(html), "index.html must contain zero inline style attributes");
  assert.ok(!/https?:\/\/(fonts|cdn|unpkg|jsdelivr)/.test(html), "index.html must not load external CDNs");
  assert.ok(!script.includes(".innerHTML"), "app.js must not reference innerHTML");
  assert.ok(!script.includes("/api/rpc") && !script.includes("clipboard"), "app.js stays read-only");
  assert.match(html, /<meta\s+name="color-scheme"\s+content="dark">/);
  assert.match(css, /:root\s*\{[\s\S]*?color-scheme:\s*dark;/);
  assert.match(css, /select\s+option[\s\S]*?background-color:\s*#101624;[\s\S]*?color:\s*#f1f5f9;/);
  assert.ok(!/\.perspective-wrap\s+select\s*\{[^}]*background:\s*transparent/.test(css), "perspective select must not use transparent background");
});

test("overview renders Pillar 1 personal quota, Pillar 2 pool accounts with live inflight, and Pillar 3 errors with meanings", () => {
  const store = new Map();
  const { context, elements } = setup(undefined, store);
  vm.runInContext(
    `snapshot = ${JSON.stringify(fixture)}; liveState = { available: true, total: 2, accounts: { "acct-aaaaaaaaaaaa": 2 } }; renderActiveView();`,
    context
  );
  // Before selecting a member, onboarding chips appear cleanly
  assert.match(elements.get("personal-cabin-body").textContent, /张测试/);
  // Select member -> persists in localStorage and renders gauge + $115.40 + release +$0.25
  vm.runInContext('selectMember("key-bbbbbbbbbbbb");', context);
  assert.equal(store.get("stats_selected_member_key_id"), "key-bbbbbbbbbbbb");
  const personalText = elements.get("personal-cabin-body").textContent;
  assert.match(personalText, /\$115\.40/);
  assert.match(personalText, /\$124\.60/);
  assert.match(personalText, /\+\$0\.25/);

  // Pillar 2 GPT shared pool shows group, 62.0%, 2/5 live concurrency
  const poolText = elements.get("pool-cabin-body").textContent;
  assert.match(poolText, /62\.0%/);
  assert.match(poolText, /2\/5/);
  assert.match(poolText, /\$637\.19/);

  // Pillar 3 digest shows plain-Chinese meaning and action
  const digestText = elements.get("overview-error-digest").textContent;
  assert.match(digestText, /共享号池并发打满/);
  assert.match(digestText, /3 个共享账号当前并发槽位已满/);
  assert.match(digestText, /等待 3~10 秒后重试/);
});

test("errors view supports 14-day selection, layer filtering, and member-only toggle", () => {
  const { context, elements } = setup();
  vm.runInContext(
    `snapshot = ${JSON.stringify(fixture)}; selectedKeyId = "key-bbbbbbbbbbbb"; selectedErrorDate = "2026-11-14"; renderErrorsView();`,
    context
  );
  assert.match(elements.get("errors-signature-list").textContent, /HTTP 503/);
  assert.match(elements.get("failure-table-body").textContent, /张测试/);

  vm.runInContext('selectedErrorDate = "2026-11-13"; renderErrorsView();', context);
  assert.match(elements.get("errors-sig-heading").textContent, /2026-11-13（昨天）/);
});

test("late dashboard response after logout cannot reveal app shell", async () => {
  let resolveDash;
  const { context, elements } = setup((path) => {
    if (path === "/api/dashboard") return new Promise((resolve) => { resolveDash = resolve; });
    if (path === "/api/logout") return Promise.resolve({ ok: true, json: async () => ({ ok: true }) });
    return Promise.resolve({ ok: true, json: async () => ({}) });
  });
  const pending = vm.runInContext('csrfToken = "test-csrf"; authGeneration = 1; loadDashboard();', context);
  vm.runInContext("handleLogout();", context);
  resolveDash({ ok: true, json: async () => ({ data: fixture, stale: false, age_seconds: 5 }) });
  await pending;
  assert.equal(elements.get("app-shell").hidden, true);
  assert.equal(elements.get("login-panel").hidden, false);
  assert.equal(elements.get("personal-cabin-body").children.length, 0);
});

test("groups preserve zero-use groups, model catalog stays literal text, and null price never shows $0", () => {
  const { context, elements } = setup();
  vm.runInContext(`snapshot = ${JSON.stringify(fixture)}; renderUsageView(); renderModelsView();`, context);
  const groupsText = elements.get("groups-table-body").textContent;
  assert.match(groupsText, /HPC\+AI（大一统）/);
  assert.match(groupsText, /零用量组/);

  const modelsText = elements.get("model-table-body").textContent;
  assert.match(modelsText, /<img/);
  assert.match(modelsText, /\$10\.00/);
  assert.match(modelsText, /未配置/);
});

test("terminal quota modal opens from topbar & personal cabin and toggles origin cleanly", async () => {
  const { context, elements } = setup((path) => {
    if (path === "/api/session") return Promise.resolve({ ok: false, json: async () => ({}) });
    return Promise.resolve({ ok: true, json: async () => ({}) });
  });
  await vm.runInContext("bootstrap()", context);
  vm.runInContext(`snapshot = ${JSON.stringify(fixture)}; selectMember("key-bbbbbbbbbbbb");`, context);

  assert.equal(elements.get("terminal-modal").hidden, true);
  elements.get("terminal-guide-button").listeners.click();
  assert.equal(elements.get("terminal-modal").hidden, false);

  const modalText = elements.get("terminal-modal-body").textContent;
  assert.match(modalText, /张测试/);
  assert.match(modalText, /https:\/\/codex\.xiaomingwiki\.cn/);
  assert.match(modalText, /\$TEAM_GATEWAY_ORIGIN\/v1\/usage\?pretty=1/);
  assert.match(modalText, /\/v1\/usage\?format=statusline/);
  assert.match(modalText, /\?usage/);
  assert.match(modalText, /ANTHROPIC_MODEL="gpt-6-astra\[1m\]"/);
  assert.match(modalText, /Codex CLI 和 Claude Code 都会在客户端本地拦截内置的 \/usage/);
  assert.doesNotMatch(modalText, /TEAM_GATEWAY_ORIGIN\/usage\?/);

  // Toggle to intranet entry
  elements.get("terminal-modal-body").children[0].children[0].children[1].listeners.click();
  assert.match(elements.get("terminal-modal-body").textContent, /http:\/\/172\.25\.228\.51:58760/);

  // Close & reopen from Personal Cabin CTA
  elements.get("terminal-modal-close").listeners.click();
  assert.equal(elements.get("terminal-modal").hidden, true);
  vm.runInContext("openTerminalModal();", context);
  assert.equal(elements.get("terminal-modal").hidden, false);
});
