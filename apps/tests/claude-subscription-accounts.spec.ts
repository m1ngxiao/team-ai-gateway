import { expect, test } from "@playwright/test";

test("Claude subscription login, activation, and removal stay in the separate account pool", async ({ page }) => {
  const calls: Array<{ method: string; params: Record<string, unknown> }> = [];
  let account: Record<string, unknown> | null = null;
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
          organizationUuid: "org-test", subscriptionType: "max", status: "disabled",
          sort: 0, expiresAt: 2_000_000_000, lastError: null,
        };
        result = { accountId: "claude:test", status: "disabled" };
        break;
      case "claudeAccount/updateStatus":
        account = account ? { ...account, status: params.status } : null;
        result = { ok: true };
        break;
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
  await expect(page.getByText("尚无 Claude 订阅账号，点击“添加 Claude 账号”开始登录。")).toBeVisible();
  await page.getByRole("button", { name: "添加 Claude 账号" }).click();
  const dialog = page.getByRole("dialog", { name: "登录 Claude 订阅账号" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("link", { name: "打开 Claude 授权页面" })).toHaveAttribute("href", "https://claude.com/test-authorize");
  await dialog.getByLabel("一次性授权码").fill("test-code#test-state");
  await dialog.getByRole("button", { name: "完成登录" }).click();
  await expect(page.getByText("claude@example.com")).toBeVisible();
  await expect(page.getByText("已停用", { exact: true })).toBeVisible();
  expect(calls.find((call) => call.method === "claudeAccount/loginComplete")?.params).toMatchObject({
    loginId: "test-state", code: "test-code#test-state",
  });

  await page.getByRole("switch", { name: "启用 Claude 账号 claude@example.com" }).click();
  await expect(page.getByText("轮转中", { exact: true })).toBeVisible();
  expect(calls.find((call) => call.method === "claudeAccount/updateStatus")?.params).toMatchObject({
    accountId: "claude:test", status: "active",
  });

  await page.getByRole("button", { name: "删除 Claude 账号 claude@example.com" }).click();
  const confirm = page.getByRole("dialog", { name: "删除 Claude 账号" });
  await confirm.getByRole("button", { name: "删除" }).click();
  await expect(page.getByText("尚无 Claude 订阅账号，点击“添加 Claude 账号”开始登录。")).toBeVisible();
  expect(calls.find((call) => call.method === "claudeAccount/delete")?.params).toMatchObject({
    accountId: "claude:test",
  });
});
