// Pure JavaScript unit tests; no browser, network, credentials, or real account data.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

const script = readFileSync(new URL("../dashboard/static/app.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../dashboard/static/index.html", import.meta.url), "utf8");
class Element {
  constructor(tag = "div") { this.tag = tag; this.children = []; this.attrs = {}; this.dataset = {}; this.listeners = {}; this.hidden = false; this.ownText = ""; }
  append(...items) { this.children.push(...items); }
  replaceChildren(...items) { this.children = items; }
  get firstChild() { return this.children[0]; }
  get textContent() { return this.ownText + this.children.map((e) => e.textContent).join(""); }
  set textContent(value) { this.ownText = String(value); this.children = []; }
  set innerHTML(value) { throw new Error("Untrusted HTML insertion is not allowed"); }
  setAttribute(name, value) { this.attrs[name] = String(value); }
  removeAttribute(name) { delete this.attrs[name]; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
}
function setup(fetchImpl = () => { throw new Error("Unexpected network request"); }) {
  const elements = new Map([...html.matchAll(/id="([^"]+)"/g)].map((match) => [match[1], new Element()]));
  const nav = ["overview", "accounts", "claude-accounts", "apikeys", "models"].map((view) => Object.assign(new Element("a"), { dataset: { view } }));
  const periods = ["today", "week", "recorded"].map((period) => Object.assign(new Element("button"), { dataset: { period } }));
  const document = {
    hidden: false,
    getElementById: (id) => { assert.ok(elements.has(id), "Missing HTML element: " + id); return elements.get(id); },
    createElement: (tag) => new Element(tag), createElementNS: (_, tag) => new Element(tag),
    querySelectorAll: (query) => query === "[data-icon]" ? [] : query.includes("data-view") ? nav : query === "[data-period]" ? periods : []
  };
  const location = { hash: "" };
  const context = vm.createContext({ document, location, history: { replaceState: (_, __, hash) => { location.hash = hash; } },
    window: { addEventListener() {} }, setInterval() {}, fetch: fetchImpl, Intl, Date, Error });
  vm.runInContext(script, context);
  return { context, elements, nav, periods };
}
const usage = (tokens) => ({ requests: 3, input_tokens: tokens, cached_tokens: 20, output_tokens: 30, total_tokens: tokens, estimated_usd: .2 });
const blankUsage = () => ({ requests: 0, input_tokens: 0, cached_tokens: 0, output_tokens: 0, total_tokens: 0, estimated_usd: 0 });
const fixture = {
  generated_at: Math.floor(Date.now()/1000),
  accounts: [{ id: "acct-aaaaaaaaaaaa", label: "账号 01", plan: "pro", status: "enabled", captured_at: 1,
    primary: { minutes: 10080, remaining_percent: 31, resets_at: null }, secondary: { minutes: null, remaining_percent: null, resets_at: null } }],
  claude_accounts: [],
  keys: [{ id: "key-bbbbbbbbbbbb", label: "同事 B", status: "enabled", last_used_at: null, usage: { today: usage(100), week: usage(200), recorded: usage(300) } }],
  totals: { today: usage(100), week: usage(200), recorded: usage(300) }, models_week: [{ model: "gpt-6-astra", usage: usage(200) }],
  pool_usage: { openai: { today: usage(100), week: usage(200), recorded: usage(300) },
    claude: { today: blankUsage(), week: blankUsage(), recorded: blankUsage() },
    unattributed: { today: blankUsage(), week: blankUsage(), recorded: blankUsage() } }
};

function groupedFixture() {
  const data = structuredClone(fixture);
  data.accounts[0].group_name = "研发组 B（示例账号）";
  data.keys[0].group_name = "研发组 B（示例账号）";
  data.key_groups = [
    { id: "group-aaaaaaaaaaaa", label: "研发组 B（示例账号）", kind: "named", key_count: 6,
      usage: { today: usage(9), week: usage(300), recorded: usage(400) } },
    { id: "group-bbbbbbbbbbbb", label: "研发组 A（示例账号）", kind: "named", key_count: 5,
      usage: { today: usage(100), week: usage(100), recorded: usage(600) } },
    { id: "group-cccccccccccc", label: "零用量组", kind: "named", key_count: 1,
      usage: { today: usage(0), week: usage(0), recorded: usage(0) } }
  ];
  return data;
}

test("account pool, overview cards and platform keys display their current group", () => {
  const { context, elements } = setup();
  const data = groupedFixture();
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts(); renderUsage();`, context);
  for (const id of ["accounts", "overview-accounts", "keys"]) assert.match(elements.get(id).textContent, /研发组 B（示例账号）/);
  data.accounts[0].group_name = null; data.keys[0].group_name = null;
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts(); renderUsage();`, context);
  assert.match(elements.get("accounts").textContent, /未分组/);
  assert.match(elements.get("keys").textContent, /全部分组（未限制）/);
});

test("group totals follow the selected period, preserve zero-use groups and sort numerically", () => {
  const { context, elements, periods } = setup();
  const data = groupedFixture();
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  const expected = [["研发组 A（示例账号）", "研发组 B（示例账号）", "零用量组"],
    ["研发组 B（示例账号）", "研发组 A（示例账号）", "零用量组"], ["研发组 A（示例账号）", "研发组 B（示例账号）", "零用量组"]];
  for (const index of [0, 1, 2, 0]) {
    periods[index].listeners.click();
    const rows = elements.get("key-groups").children;
    assert.deepEqual(rows.map((row) => row.children[0].children[0].textContent), expected[index]);
    const first = data.key_groups.find((group) => group.label === expected[index][0]);
    const current = first.usage[periods[index].dataset.period];
    assert.deepEqual(rows[0].children.slice(1).map((cell) => cell.textContent),
      [first.key_count, current.requests, current.input_tokens, current.cached_tokens, current.output_tokens, current.total_tokens].map(String).concat("$0.2000"));
    assert.equal(elements.get("groups-period").textContent, ["今日", "近 7 天", "累计"][index] + " · 按 Token 用量降序");
  }
  assert.equal(vm.runInContext("JSON.stringify(snapshot.key_groups)", context), JSON.stringify(data.key_groups));
});

test("group labels cannot inject markup and special groups show their distinct meanings", () => {
  const { context, elements } = setup();
  const data = groupedFixture();
  const name = '<img src=x onerror="attack()">';
  data.accounts[0].group_name = name; data.keys[0].group_name = name;
  data.key_groups[0].label = name;
  data.key_groups[1].kind = "unrestricted"; data.key_groups[1].label = "全部分组（未限制）";
  data.key_groups[2].kind = "unattributed"; data.key_groups[2].label = "已删除或未归属 Key"; data.key_groups[2].key_count = 0;
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts(); renderUsage();`, context);
  const rows = elements.get("key-groups").children;
  const label = rows[1].children[0].children[0];
  assert.equal(label.textContent, name); assert.equal(label.children.length, 0); assert.equal(label.tag, "div");
  assert.match(rows[0].textContent, /Key 未限制账号分组/);
  assert.match(rows[2].textContent, /保留历史记录/);
  assert.equal(rows[2].children[1].textContent, "—");
});

test("old snapshots show pending group data instead of claiming zero usage", () => {
  const { context, elements } = setup();
  for (const groups of [undefined, []]) {
    const data = structuredClone(fixture); data.key_groups = groups;
    vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
    assert.match(elements.get("key-groups").textContent, /分组统计待更新/);
    assert.equal(elements.get("keys-total-tokens").textContent, "100");
  }
  const empty = structuredClone(fixture); empty.keys = [];
  empty.totals.today = Object.fromEntries(Object.keys(empty.totals.today).map((key) => [key, 0]));
  vm.runInContext(`snapshot = ${JSON.stringify(empty)}; renderUsage();`, context);
  assert.equal(elements.get("key-groups").textContent, "暂无分组用量。");
});

test("refresh updates group membership and usage without changing the chosen period", async () => {
  const before = groupedFixture(), after = groupedFixture();
  after.key_groups[1].usage.week.total_tokens = 900;
  after.key_groups[0].label = "新的组名"; after.keys[0].group_name = "新的组名";
  const { context, elements, periods } = setup(() => Promise.resolve({ ok: true, json: async () => ({ data: after, stale: false }) }));
  vm.runInContext(`snapshot = ${JSON.stringify(before)}; renderUsage();`, context);
  periods[1].listeners.click();
  await vm.runInContext("refresh()", context);
  assert.equal(elements.get("key-groups").children[0].children[0].textContent, "研发组 A（示例账号）");
  assert.match(elements.get("keys").textContent, /新的组名/);
  assert.equal(elements.get("groups-period").textContent, "近 7 天 · 按 Token 用量降序");
  vm.runInContext("showLogin()", context);
  assert.equal(elements.get("key-groups").children.length, 0);
});

test("navigation switches account-pool and usage views without network calls", () => {
  const { context, elements, nav } = setup();
  vm.runInContext('navigate("accounts")', context);
  assert.equal(elements.get("page-title").textContent, "OpenAI 账号池");
  assert.equal(elements.get("view-accounts").hidden, false);
  assert.equal(elements.get("view-overview").hidden, true);
  assert.equal(elements.get("period-controls").hidden, false);
  assert.equal(nav[1].attrs["aria-current"], "page");
  vm.runInContext('navigate("claude-accounts")', context);
  assert.equal(elements.get("page-title").textContent, "Claude 账号池");
  assert.equal(elements.get("view-claude-accounts").hidden, false);
  assert.equal(elements.get("view-accounts").hidden, true);
  assert.equal(elements.get("period-controls").hidden, false);
  assert.equal(nav[2].attrs["aria-current"], "page");
  vm.runInContext('navigate("apikeys")', context);
  assert.equal(elements.get("view-apikeys").hidden, false);
  assert.equal(elements.get("period-controls").hidden, false);
});

test("Claude subscription accounts have a separate read-only view and no invented quota", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture);
  data.claude_accounts = [
    { id: "acct-cccccccccccc", label: "Claude 01", email: "claude@example.com", plan: "pro", status: "enabled", sort_order: 0, updated_at: 1900000000 },
    { id: "acct-dddddddddddd", label: "Claude 02", email: null, plan: "max", status: "needs_login", sort_order: 1, updated_at: null }
  ];
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts();`, context);
  assert.equal(elements.get("overview-account-count").textContent, "3");
  assert.equal(elements.get("overview-enabled-count").textContent, "2");
  assert.equal(elements.get("account-count").textContent, "共 1 个账号");
  assert.equal(elements.get("claude-account-count").textContent, "共 2 个账号");
  assert.equal(elements.get("accounts").children.length, 1);
  assert.equal(elements.get("claude-accounts").children.length, 2);
  assert.match(elements.get("claude-accounts").textContent, /claude@example.com.*PRO.*启用.*暂无数据.*Claude 02.*MAX.*需重新登录/s);
  assert.match(elements.get("overview-accounts").textContent, /Claude · PRO.*暂无数据.*最近成功采集：暂无记录/s);
  assert.ok(!elements.get("accounts").textContent.includes("claude@example.com"));
  assert.match(html, /href="#claude-accounts" data-view="claude-accounts"/);
  assert.match(html, /本站 Token 用量.*不能换算为订阅剩余额度/);
});

test("Claude quota shows fresh remaining percentages and safe refresh status", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture), now = Math.floor(Date.now() / 1000);
  data.claude_accounts = [{ id: "acct-cccccccccccc", label: "Claude 01", email: "claude@example.com",
    plan: "max", status: "enabled", captured_at: now - 120, last_attempt_at: now - 60,
    next_attempt_at: now + 600, last_error: "rate_limited",
    five_hour: { minutes: 300, remaining_percent: 75.5, resets_at: now + 1800 },
    seven_day: { minutes: 10080, remaining_percent: 20, resets_at: now + 86400 * 5 } }];
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts();`, context);
  const table = elements.get("claude-accounts").textContent;
  assert.match(table, /75\.5%.*20\.0%/s);
  assert.match(table, /最近成功采集.*最近查询.*查询限流.*下次尝试/s);
  assert.match(elements.get("overview-accounts").textContent, /Claude · MAX.*75\.5%.*20\.0%/s);
});

test("Claude expired quota and elapsed reset are unknown even when gateway tokens exist", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture), now = Math.floor(Date.now() / 1000);
  data.claude_accounts = [{ id: "acct-cccccccccccc", label: "Claude 01", plan: "pro", status: "enabled",
    captured_at: now - 7200, five_hour: { minutes: 300, remaining_percent: 80, resets_at: now + 1800 },
    seven_day: { minutes: 10080, remaining_percent: 90, resets_at: now + 86400 } }];
  data.pool_usage.claude.today.total_tokens = 500000;
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts();`, context);
  let shown = elements.get("claude-accounts").textContent;
  assert.match(shown, /最近额度已过期，当前额度未知/);
  assert.match(shown, /未知.*未知/s);
  assert.ok(!shown.includes("80.0%") && !shown.includes("90.0%") && !shown.includes("500000"));
  data.claude_accounts[0].captured_at = now - 60;
  data.claude_accounts[0].five_hour.resets_at = now - 1;
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts();`, context);
  shown = elements.get("claude-accounts").textContent;
  assert.ok(!shown.includes("80.0%"));
  assert.match(shown, /未知/);
  assert.match(shown, /90\.0%/);
});

test("actual-source pool usage reconciles with team totals and changes with period", () => {
  const { context, elements, periods } = setup();
  const data = structuredClone(fixture);
  data.pool_usage = {
    openai: { today: usage(70), week: usage(120), recorded: usage(170) },
    claude: { today: usage(20), week: usage(40), recorded: usage(60) },
    unattributed: { today: usage(10), week: usage(40), recorded: usage(70) }
  };
  data.keys[0].upstream_provider = "claude";
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  assert.equal(elements.get("keys").children[0].children[1].textContent, "Claude");
  assert.equal(elements.get("overview-claude-tokens").textContent, "20");
  assert.equal(elements.get("overview-unattributed-tokens").textContent, "10");
  assert.equal(elements.get("claude-pool-usage").textContent, "20 Token");
  periods[1].listeners.click();
  assert.equal(elements.get("overview-openai-tokens").textContent, "120");
  assert.equal(elements.get("overview-claude-tokens").textContent, "40");
  assert.equal(elements.get("overview-unattributed-tokens").textContent, "40");
  assert.equal(elements.get("total-tokens").textContent, "200");
  assert.match(html, /聚合 API、旧记录及无法区分来源的历史汇总计入未归属/);
  assert.match(html, /上游厂商/);
});

test("old snapshots wait for Claude account and pool data instead of claiming zero", () => {
  const { context, elements } = setup();
  const old = structuredClone(fixture);
  delete old.claude_accounts;
  delete old.pool_usage;
  vm.runInContext(`snapshot = ${JSON.stringify(old)}; renderAccounts(); renderUsage();`, context);
  assert.equal(elements.get("overview-account-count").textContent, "—");
  assert.equal(elements.get("claude-account-count").textContent, "账号数量待更新");
  assert.match(elements.get("claude-accounts").textContent, /等待新版采集器统计/);
  assert.equal(elements.get("overview-claude-tokens").textContent, "—");
  assert.equal(elements.get("claude-pool-usage").textContent, "统计待更新");
  assert.equal(elements.get("total-tokens").textContent, "100");
  // The new server validates old JSON and serializes default values.
  old.claude_accounts = []; old.pool_usage = null;
  vm.runInContext(`snapshot = ${JSON.stringify(old)}; renderAccounts(); renderUsage();`, context);
  assert.equal(elements.get("claude-account-count").textContent, "账号数量待更新");
  assert.equal(elements.get("overview-account-count").textContent, "—");
});

test("invalid hash never opens an admin/settings view", () => {
  const { context, elements } = setup();
  vm.runInContext('navigate("/admin/settings")', context);
  assert.equal(elements.get("page-title").textContent, "仪表盘");
});

test("account and usage views render safe fields and period summaries agree", () => {
  const { context, elements, periods } = setup();
  vm.runInContext(`snapshot = ${JSON.stringify(fixture)}; renderAccounts(); renderUsage(); renderModels();`, context);
  assert.equal(elements.get("overview-account-count").textContent, "1");
  assert.match(elements.get("accounts").textContent, /31\.0%/);
  assert.match(elements.get("accounts").textContent, /不单独限额/);
  assert.match(elements.get("keys").textContent, /key-bbbbbbbbbbbb/);
  assert.match(elements.get("keys").textContent, /仅查看/);
  periods[2].listeners.click();
  assert.equal(elements.get("total-tokens").textContent, "300");
  assert.equal(elements.get("keys-total-tokens").textContent, "300");
});

test("platform key names render above their identifiers without exposing secrets", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture);
  data.keys[0].display_id = "gk_123456789abc";
  data.keys[0].api_key = "secret-must-not-render";
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  const cell = elements.get("keys").children[0].children[0];
  assert.equal(cell.children[0].textContent, "同事 B");
  assert.equal(cell.children[0].className, "key-label");
  assert.equal(cell.children[1].textContent, "gk_123456789abc");
  assert.ok(!elements.get("keys").textContent.includes("secret-must-not-render"));
  assert.equal(elements.get("keys").children[0].children.length, 9);
});

test("key rows sort numerically by total tokens in the selected period", () => {
  const { context, elements, periods } = setup();
  const data = structuredClone(fixture);
  data.keys = [
    { ...structuredClone(fixture.keys[0]), id: "key-aaaaaaaaaaaa", label: "同事 A",
      usage: { today: usage(9), week: usage(3000), recorded: usage(4000) } },
    { ...structuredClone(fixture.keys[0]), id: "key-bbbbbbbbbbbb", label: "同事 B",
      usage: { today: usage(100), week: usage(1000), recorded: usage(6000) } },
    { ...structuredClone(fixture.keys[0]), id: "key-cccccccccccc", label: "同事 C",
      usage: { today: usage(20), week: usage(2000), recorded: usage(5000) } }
  ];
  // Do not sort by price, cached/input tokens, or formatted abbreviations.
  data.keys[0].usage.today.estimated_usd = 100;
  data.keys[0].usage.today.input_tokens = 9999;
  data.keys[0].usage.today.cached_tokens = 9999;
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  const names = () => elements.get("keys").children.map((row) => row.children[0].children[0].textContent);
  const expected = [["同事 B", "同事 C", "同事 A"], ["同事 A", "同事 C", "同事 B"], ["同事 B", "同事 C", "同事 A"]];
  assert.deepEqual(names(), expected[0]);
  for (const index of [0, 1, 2, 0]) {
    periods[index].listeners.click();
    assert.deepEqual(names(), expected[index]);
    assert.equal(elements.get("keys-sort-order").textContent,
      "按" + ["今日", "近 7 天", "累计"][index] + " Token 用量从高到低排列");
    assert.equal(elements.get("keys-total-tokens").title,
      new Intl.NumberFormat("zh-CN").format(data.totals[periods[index].dataset.period].total_tokens));
  }
  assert.equal(vm.runInContext("JSON.stringify(snapshot.keys)", context), JSON.stringify(data.keys));
  assert.match(html, /aria-sort="descending" aria-describedby="keys-sort-order"/);
});

test("sorting preserves tied key order and keeps zero-use and disabled keys visible", () => {
  const { context, elements, periods } = setup();
  const data = structuredClone(fixture);
  data.keys = [
    { ...structuredClone(fixture.keys[0]), id: "key-aaaaaaaaaaaa", label: "零用量",
      usage: { today: usage(0), week: usage(0), recorded: usage(0) } },
    { ...structuredClone(fixture.keys[0]), id: "key-bbbbbbbbbbbb", label: "并列 B", status: "disabled" },
    { ...structuredClone(fixture.keys[0]), id: "key-cccccccccccc", label: "并列 C" },
    { ...structuredClone(fixture.keys[0]), id: "key-dddddddddddd", label: "历史密钥", is_historical: true,
      usage: { today: usage(9999), week: usage(9999), recorded: usage(9999) } }
  ];
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  for (const control of periods) {
    control.listeners.click();
    const rows = elements.get("keys").children;
    assert.deepEqual(rows.map((row) => row.children[0].children[0].textContent), ["并列 B", "并列 C", "零用量"]);
    assert.match(rows[0].textContent, /停用/);
    assert.ok(!elements.get("keys").textContent.includes("历史密钥"));
  }
});

test("refresh reorders key rows using updated usage without resetting the selected period", async () => {
  const data = structuredClone(fixture);
  data.keys.push({ ...structuredClone(data.keys[0]), id: "key-cccccccccccc", label: "同事 C",
    usage: { today: usage(0), week: usage(100), recorded: usage(100) } });
  const updated = structuredClone(data);
  updated.keys[1].usage.week.total_tokens = 900;
  const { context, elements, periods } = setup((path) => {
    assert.equal(path, "/api/dashboard");
    return Promise.resolve({ ok: true, json: async () => ({ data: updated, stale: false }) });
  });
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  periods[1].listeners.click();
  assert.equal(elements.get("keys").children[0].children[0].children[0].textContent, "同事 B");
  await vm.runInContext("refresh()", context);
  assert.equal(elements.get("keys").children[0].children[0].children[0].textContent, "同事 C");
  assert.equal(elements.get("keys-sort-order").textContent, "按近 7 天 Token 用量从高到低排列");
  assert.equal(periods[1].attrs["aria-pressed"], "true");
});

test("old snapshots with missing or invalid names fall back to identifiers", () => {
  for (const label of [undefined, null, "", "   ", 123, {}]) {
    const { context, elements } = setup();
    const data = structuredClone(fixture);
    data.keys[0].label = label;
    vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
    const cell = elements.get("keys").children[0].children[0];
    assert.equal(cell.textContent, "key-bbbbbbbbbbbb");
    assert.equal(cell.children.length, 1);
  }
});

test("key names containing markup render only as literal text", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture);
  data.keys[0].label = '<img src=x onerror="attack()">';
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  const cell = elements.get("keys").children[0].children[0];
  assert.equal(cell.children[0].textContent, data.keys[0].label);
  assert.equal(cell.children[0].tag, "div");
  assert.equal(cell.children[0].children.length, 0);
});

test("long key names remain readable and can wrap within the identity column", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture);
  data.keys[0].label = "团队成员长名称".repeat(18);
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  assert.equal(elements.get("keys").children[0].children[0].children[0].textContent, data.keys[0].label);
  const css = readFileSync(new URL("../dashboard/static/app.css", import.meta.url), "utf8");
  assert.match(css, /\.key-table \.key-label\s*\{[^}]*max-width:\s*260px[^}]*overflow-wrap:\s*anywhere/);
});

test("historical keys in old snapshots are hidden without changing team totals", () => {
  const { context, elements, periods } = setup();
  const data = structuredClone(fixture);
  data.keys.push({ ...structuredClone(data.keys[0]), id: "key-cccccccccccc", is_historical: true, status: "disabled" });
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts(); renderUsage();`, context);
  assert.equal(elements.get("overview-key-count").textContent, "1");
  assert.equal(elements.get("keys").children.length, 1);
  for (const control of periods) {
    control.listeners.click();
    assert.ok(!elements.get("keys").textContent.includes("key-cccccccccccc"));
    assert.ok(!elements.get("keys").textContent.includes("历史"));
    const expected = String(data.totals[control.dataset.period].total_tokens);
    assert.equal(elements.get("total-tokens").textContent, expected);
    assert.equal(elements.get("keys-total-tokens").textContent, expected);
  }
});

test("historical-only snapshots show zero keys and an empty state", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture);
  data.keys[0].is_historical = true;
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts(); renderUsage();`, context);
  assert.equal(elements.get("overview-key-count").textContent, "0");
  assert.equal(elements.get("keys").textContent, "暂无密钥记录。");
  assert.equal(elements.get("keys").children[0].children[0].colSpan, 9);
  assert.equal(elements.get("total-tokens").textContent, "100");
});

test("current disabled and unused keys are not mistaken for historical keys", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture);
  Object.assign(data.keys[0], { status: "disabled", is_historical: false,
    usage: { today: usage(0), week: usage(0), recorded: usage(0) } });
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts(); renderUsage();`, context);
  assert.equal(elements.get("overview-key-count").textContent, "1");
  assert.equal(elements.get("keys").children.length, 1);
  assert.match(elements.get("keys").textContent, /key-bbbbbbbbbbbb/);
  assert.match(elements.get("keys").textContent, /停用/);
});

test("late statistics response cannot reveal the page after logout", async () => {
  let resolveStats;
  const { context, elements } = setup((path) => {
    if (path === "/api/dashboard") return new Promise((resolve) => { resolveStats = resolve; });
    if (path === "/api/logout") return Promise.resolve({ ok: true, json: async () => ({ ok: true }) });
    throw new Error("Unexpected endpoint");
  });
  const pending = vm.runInContext('csrf = "unit-test-csrf"; refresh()', context);
  await elements.get("logout").listeners.click();
  resolveStats({ ok: true, json: async () => ({ data: fixture, stale: false }) });
  await pending;
  assert.equal(elements.get("dashboard").hidden, true);
  assert.equal(elements.get("login-panel").hidden, false);
  assert.equal(elements.get("keys").children.length, 0);
});

test("administrator controls have no active management transport", () => {
  for (const label of ["导入账号", "添加账号", "创建密钥"]) {
    const button = [...html.matchAll(/<button\b([^>]*)>([\s\S]*?)<\/button>/g)].find((match) => match[2].includes(label));
    assert.ok(button && /\bdisabled\b/.test(button[1]), label + " must be disabled");
  }
  assert.ok(!script.includes("/api/rpc") && !script.includes("readSecret") && !script.includes("clipboard"));
  assert.match(html, /密钥名称/);
});

test("pool details show email, expiry, separate Spark quotas and read-only reset credits", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture);
  Object.assign(data.accounts[0], { email: "user@example.com", sort_order: 0, proxy_state: "inherited",
    subscription_expires_at: 1900000000, reset_available_count: 3, capacity_override: false,
    spark_primary: { minutes: 300, remaining_percent: 100, resets_at: 1900000000 },
    spark_secondary: { minutes: 10080, remaining_percent: 90, resets_at: 1900000000 } });
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderAccounts();`, context);
  const text = elements.get("accounts").textContent;
  for (const expected of ["user@example.com", "订阅到期", "Spark 额度", "长周期", "100.0%", "90.0%", "31.0%", "仅 7 天额度", "可用重置次数：3"]) assert.ok(text.includes(expected), expected);
  for (const removed of ["快照", "未设置账号容量覆盖"]) assert.ok(!text.includes(removed), removed);
  const flatten = (element) => [element, ...element.children.flatMap(flatten)];
  assert.equal(flatten(elements.get("accounts")).filter((element) => ["button", "input"].includes(element.tag)).length, 0);
  assert.equal(elements.get("accounts").children[0].children.length, 6);
});

test("countdown does not reset quota and expired data is explicitly pending", () => {
  const { context } = setup();
  assert.equal(vm.runInContext('countdownText(100, 101)', context), "等待额度更新");
  assert.equal(vm.runInContext('countdownText(null, 100)', context), "等待更新");
  assert.equal(vm.runInContext('countdownText(100 + 90060, 100)', context), "1 天 1 时 1 分后重置");
  assert.match(vm.runInContext('renderWindow({ remaining_percent: 8, resets_at: 1, minutes: 10080 }, "周额度", 1).textContent', context), /8.0%.*等待额度更新/);
});

test("platform keys show original metadata and retain detailed usage without secret actions", () => {
  const { context, elements } = setup();
  const data = structuredClone(fixture);
  Object.assign(data.keys[0], { display_id: "gk_123456789abc", protocol: "openai_compat", rotation: "account_rotation",
    model_binding: "request", quota_config_known: true, quota_limit_tokens: 250 });
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderUsage();`, context);
  for (const expected of ["gk_123456789abc", "OpenAI 兼容", "账号轮转", "跟随请求", "今日", "累计", "限额 250", "剩余 0", "用量明细", "仅查看"]) assert.ok(elements.get("keys").textContent.includes(expected), expected);
  const flatten = (element) => [element, ...element.children.flatMap(flatten)];
  assert.equal(flatten(elements.get("keys")).filter((element) => ["input", "button"].includes(element.tag)).length, 0);
  assert.ok(flatten(elements.get("keys")).some((element) => element.tag === "details"));
});

const catalogFixture = [
  { model: "gpt-6-astra", name: "GPT-6-Astra", description: "Complex work", enabled: true, origin: "builtin", supported_in_api: true,
    instructions_mode: "passthrough", route_count: 2, account_pool_routes: 1, aggregate_api_routes: 1,
    price: { status: "official", input_usd_per_million: 10, cached_input_usd_per_million: 1, cache_write_usd_per_million: 12.5, output_usd_per_million: 50 } },
  { model: "gpt-5.6-sol", name: "GPT-5.6-Sol", description: "Coding", enabled: false, origin: "builtin", supported_in_api: false,
    instructions_mode: "override", route_count: 0, account_pool_routes: 0, aggregate_api_routes: 0, price: { status: "missing" } }
];

test("model directory navigation, search and status filter only read local data", () => {
  const { context, elements, nav } = setup();
  const data = { ...fixture, model_catalog: catalogFixture, catalog_unlisted_count: 1 };
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderCatalog(); navigate("models");`, context);
  assert.equal(elements.get("view-models").hidden, false);
  assert.equal(elements.get("period-controls").hidden, true);
  assert.equal(nav[4].attrs["aria-current"], "page");
  assert.equal(elements.get("catalog-rows").children.length, 2);
  assert.match(elements.get("catalog-rows").textContent, /10 \/ 1 \/ 12.5 \/ 50/);
  assert.match(elements.get("catalog-rows").textContent, /账号池 × 1聚合 API × 1/);
  assert.match(elements.get("catalog-rows").textContent, /价格未提供— \/ — \/ — \/ —/);
  assert.equal(elements.get("catalog-unlisted").hidden, false);
  elements.get("catalog-search").value = "ASTRA";
  elements.get("catalog-search").listeners.input();
  assert.equal(elements.get("catalog-rows").children.length, 1);
  assert.match(elements.get("catalog-count").textContent, /1 \/ 2/);
  elements.get("catalog-search").value = "";
  elements.get("catalog-status").value = "disabled";
  elements.get("catalog-status").listeners.change();
  assert.match(elements.get("catalog-rows").textContent, /GPT-5.6-Sol/);
  assert.ok(!elements.get("catalog-rows").textContent.includes("GPT-6-Astra"));
  vm.runInContext('showLogin()', context);
  assert.equal(elements.get("catalog-rows").children.length, 0);
});

test("catalogue descriptions are text and unconfigured prices are not zero", () => {
  const { context, elements } = setup();
  const data = { ...fixture, model_catalog: structuredClone(catalogFixture) };
  data.model_catalog[0].description = '<img src=x onerror="attack()">';
  data.model_catalog[0].price.cache_write_usd_per_million = 0;
  vm.runInContext(`snapshot = ${JSON.stringify(data)}; renderCatalog();`, context);
  assert.match(elements.get("catalog-rows").textContent, /<img/);
  assert.match(elements.get("catalog-rows").textContent, /10 \/ 1 \/ 0 \/ 50/);
  assert.match(elements.get("catalog-rows").textContent, /价格未提供— \/ — \/ — \/ —/);
});

test("weekly-only Pro has no separate 5h card limit, but missing data is not unlimited", () => {
  const { context } = setup();
  const pro = structuredClone(fixture.accounts[0]);
  const render = (account) => vm.runInContext(`accountWindows(${JSON.stringify(account)}).textContent`, context);
  assert.match(render(pro), /5 小时不单独限额按周额度使用/);
  const reversed = { ...pro, primary: pro.secondary, secondary: pro.primary };
  assert.match(render(reversed), /不单独限额/);
  assert.ok(!render({ ...pro, plan: "plus" }).includes("不单独限额"));
  assert.ok(!render({ ...pro, primary: pro.secondary }).includes("不单独限额"));
  assert.ok(!render({ ...pro, captured_at: null }).includes("不单独限额"));
  const explicit = { ...reversed, primary: { minutes: 300, remaining_percent: 45, resets_at: 1900000000 } };
  assert.ok(!render(explicit).includes("不单独限额"));
  assert.match(render(explicit), /45.0%/);
  const signalOnly = { ...pro, secondary: { minutes: null, remaining_percent: null, resets_at: 1900000000 } };
  assert.ok(!render(signalOnly).includes("不单独限额"));
  const partialPlus = { ...explicit, plan: "plus", primary: { minutes: 300, remaining_percent: null, resets_at: null } };
  assert.equal(vm.runInContext(`accountStatus(${JSON.stringify(partialPlus)}).textContent`, context), "启用");
});

test("team branding and concise copy replace admin-facing footnotes", () => {
  assert.match(html, /<title>Team AI Gateway · 团队 AI 用量<\/title>/);
  assert.equal((html.match(/<strong>Team AI Gateway<\/strong>/g) || []).length, 2);
  assert.equal((html.match(/class="brand-icon">AI/g) || []).length, 2);
  for (const phrase of ["CodexManager", "快照", "前 6 个自然日", "额度未知不等于", "不能精确换算", "TOKEN / 金额</th><th scope=\"col\">名称"]) assert.ok(!html.includes(phrase), phrase);
  assert.ok(!html.includes("<footer>"));
  assert.match(html, /显示名称和编号，不显示完整密钥/);
  assert.match(html, /美元 \/ 百万 Token/);
});
