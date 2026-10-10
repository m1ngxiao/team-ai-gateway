import { expect, test } from "@playwright/test";

test("Claude subscription login, activation, and removal stay in the separate account pool", async ({ page }) => {
  const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
  const nowSeconds = Math.floor(Date.now() / 1000);
  let account: Record<string, unknown> | null = null;
  let usageRefreshCount = 0;
  await page.route("**/api/runtime**", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({ mode: "web-gateway", rpcBaseUrl: "/api/rpc", canManageService: false }),
    });
  });
  await page.route("**/api/rpc**", async (route) => {
    const request = route.request().postDataJSON() as {
      id: number;
      method: string;
      params?: Record<string, unknown>;
    };
    const params = request.params ?? {};
    calls.push({ method: request.method, params });
    let result: unknown = {};
    switch (request.method) {
      case "appSettings/get":
        result = {
          locale: "zh-CN", serviceAddr: "localhost:48760", theme: "tech",
          appearancePreset: "classic", codexCliGuideDismissed: true,
        };
        break;
      case "initialize":
        result = {
          userAgent: "codex_cli_rs/0.1.19", codexHome: "/tmp/test-codex",
          platformFamily: "linux", platformOs: "linux",
        };
        break;
      case "accountManager/session/current":
        result = {
          mode: "none", currentUser: null, role: "system_admin",
          permissions: ["system:admin"], distributionEnabled: false,
        };
        break;
      case "account/list":
        result = { items: [], total: 0, page: 1, pageSize: 20 };
        break;
      case "account/usage/list":
        result = [];
        break;
      case "claudeAccount/list":
        result = account ? [account] : [];
        break;
      case "claudeAccount/loginStart":
        result = { loginId: "test-state", authUrl: "https://claude.com/test-authorize" };
        break;
      case "claudeAccount/loginComplete":
        account = {
          id: "claude:test", label: "claude@example.com", email: "claude@example.com",
          organizationUuid: "org-test", subscriptionType: "claude_max_5x", status: "disabled",
          sort: 0, expiresAt: 2_000_000_000, lastError: null,
        };
        result = { accountId: "claude:test", status: "disabled" };
        break;
      case "claudeAccount/updateStatus":
        account = account ? { ...account, status: params.status } : null;
        result = { ok: true };
        break;
      case "claudeAccount/usageRefresh": {
        usageRefreshCount += 1;
        const usage = usageRefreshCount <= 2
          ? {
              fiveHour: { usedPercent: 42.5, resetsAt: nowSeconds + 3_600 },
              sevenDay: { usedPercent: 31, resetsAt: nowSeconds + 300_000 },
              capturedAt: nowSeconds, lastAttemptAt: nowSeconds,
              nextAttemptAt: nowSeconds + 300, lastError: null,
            }
          : usageRefreshCount === 3 ? {
              fiveHour: { usedPercent: 42.5, resetsAt: nowSeconds + 3_600 },
              sevenDay: { usedPercent: 31, resetsAt: nowSeconds + 300_000 },
              capturedAt: nowSeconds - 3_601, lastAttemptAt: nowSeconds,
              nextAttemptAt: nowSeconds + 300, lastError: "Claude usage temporarily unavailable",
            } : {
              fiveHour: { usedPercent: 42.5, resetsAt: nowSeconds - 1 },
              sevenDay: { usedPercent: 31, resetsAt: nowSeconds + 300_000 },
              capturedAt: nowSeconds, lastAttemptAt: nowSeconds,
              nextAttemptAt: nowSeconds + 300, lastError: null,
            };
        account = account ? { ...account, usage } : null;
        result = usage;
        break;
      }
      case "claudeAccount/delete":
        account = null;
        result = { ok: true };
        break;
    }
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({ jsonrpc: "2.0", id: request.id, result }),
    });
  });

  await page.goto("/accounts/");
  await expect(page.getByRole("heading", { name: "OpenAI 账号池" })).toBeVisible();
  await expect(page.getByText("尚无 Claude 订阅账号，点击“添加 Claude 账号”开始登录。")).toHaveCount(0);
  await page.getByRole("link", { name: "Claude 账号池" }).click();
  await expect(page).toHaveURL(/\/claude-accounts\/$/);
  await expect(page.getByRole("heading", { name: "Claude 账号池" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "OpenAI 账号池" })).toHaveCount(0);
  await expect(page.getByText("尚无 Claude 订阅账号，点击“添加 Claude 账号”开始登录。")).toBeVisible();
  await page.getByRole("button", { name: "添加 Claude 账号" }).click();
  const dialog = page.getByRole("dialog", { name: "登录 Claude 订阅账号" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("link", { name: "打开 Claude 授权页面" })).toHaveAttribute("href", "https://claude.com/test-authorize");
  await dialog.getByLabel("一次性授权码").fill("test-code#test-state");
  await dialog.getByRole("button", { name: "完成登录" }).click();
  await expect(page.getByText("claude@example.com")).toBeVisible();
  await expect(page.getByText("Max", { exact: true })).toBeVisible();
  await expect(page.getByText("claude_max_5x", { exact: true })).toHaveCount(0);
  await expect(page.getByText("已停用", { exact: true }).last()).toBeVisible();
  await expect(page.getByText("暂无额度数据", { exact: true })).toBeVisible();
  await expect(page.getByText("5 小时已用:")).toBeVisible();
  await expect(page.getByRole("button", { name: "刷新 Claude 额度 claude@example.com" })).toBeDisabled();
  expect(calls.find((call) => call.method === "claudeAccount/loginComplete")?.params).toMatchObject({
    loginId: "test-state", code: "test-code#test-state",
  });

  await page.getByRole("switch", { name: "启用 Claude 账号 claude@example.com" }).click();
  await expect(page.getByText("轮转中", { exact: true }).last()).toBeVisible();
  await expect(page.getByRole("switch", { name: "停用 Claude 账号 claude@example.com" })).toBeChecked();
  expect(calls.find((call) => call.method === "claudeAccount/updateStatus")?.params).toMatchObject({
    accountId: "claude:test", status: "active",
  });

  await page.getByRole("button", { name: "刷新 Claude 额度 claude@example.com" }).click();
  await expect(page.getByText("42.5%", { exact: true })).toBeVisible();
  await expect(page.getByText("31%", { exact: true })).toBeVisible();
  await expect(page.getByText("已更新", { exact: true })).toBeVisible();
  await expect(page.getByText(/最近观测于/)).toBeVisible();
  await expect(page.getByText(/上次查询/)).toBeVisible();
  await expect(page.getByText(/下次尝试/)).toBeVisible();
  expect(calls.find((call) => call.method === "claudeAccount/usageRefresh")?.params).toMatchObject({
    accountId: "claude:test",
  });

  await page.getByRole("button", { name: "刷新 Claude 额度 claude@example.com" }).click();
  await expect(page.getByText("已是最新额度数据", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "刷新 Claude 额度 claude@example.com" }).click();
  await expect(page.getByText("查询失败", { exact: true })).toBeVisible();
  await expect(page.getByText("数据过期", { exact: true })).toBeVisible();
  await expect(page.getByRole("main").getByText("Claude usage temporarily unavailable", { exact: true })).toBeVisible();
  await expect(page.getByText("42.5%", { exact: true })).toHaveCount(0);
  await expect(page.getByText("31%", { exact: true })).toHaveCount(0);
  await expect(page.getByText("5 小时已用:").locator("strong")).toHaveText("—");
  await expect(page.getByText("7 天已用:").locator("strong")).toHaveText("—");
  await expect(page.getByText(/最近观测于/)).toBeVisible();

  await page.getByRole("button", { name: "刷新 Claude 额度 claude@example.com" }).click();
  await expect(page.getByText("42.5%", { exact: true })).toHaveCount(0);
  await expect(page.getByText("31%", { exact: true })).toBeVisible();
  await expect(page.getByText("5 小时已用:").locator("strong")).toHaveText("—");
  await expect(page.getByText("数据过期", { exact: true })).toBeVisible();
  await expect(page.getByText(/最近观测于/)).toBeVisible();

  await page.getByRole("button", { name: "删除 Claude 账号 claude@example.com" }).click();
  const confirm = page.getByRole("dialog", { name: "删除 Claude 账号" });
  await confirm.getByRole("button", { name: "删除" }).click();
  await expect(page.getByText("尚无 Claude 订阅账号，点击“添加 Claude 账号”开始登录。")).toBeVisible();
  expect(calls.find((call) => call.method === "claudeAccount/delete")?.params).toMatchObject({
    accountId: "claude:test",
  });
});
