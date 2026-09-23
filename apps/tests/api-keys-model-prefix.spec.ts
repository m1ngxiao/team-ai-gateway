import { expect, test } from "@playwright/test";

const SETTINGS_SNAPSHOT = {
  updateAutoCheck: true,
  closeToTrayOnClose: false,
  closeToTraySupported: false,
  lowTransparency: false,
  lightweightModeOnCloseToTray: false,
  codexCliGuideDismissed: true,
  webAccessPasswordConfigured: false,
  locale: "zh-CN",
  localeOptions: ["zh-CN", "en"],
  serviceAddr: "localhost:48760",
  serviceListenMode: "loopback",
  serviceListenModeOptions: ["loopback", "all_interfaces"],
  routeStrategy: "ordered",
  routeStrategyOptions: ["ordered", "balanced"],
  freeAccountMaxModel: "auto",
  freeAccountMaxModelOptions: ["auto", "gpt-5"],
  modelForwardRules: "",
  accountMaxInflight: 1,
  gatewayOriginator: "codex-cli",
  gatewayOriginatorDefault: "codex-cli",
  gatewayUserAgentVersion: "1.0.0",
  gatewayUserAgentVersionDefault: "1.0.0",
  gatewayResidencyRequirement: "",
  gatewayResidencyRequirementOptions: ["", "us"],
  pluginMarketMode: "builtin",
  pluginMarketSourceUrl: "",
  upstreamProxyUrl: "",
  upstreamStreamTimeoutMs: 600000,
  sseKeepaliveIntervalMs: 15000,
  backgroundTasks: {
    usagePollingEnabled: true,
    usagePollIntervalSecs: 600,
    gatewayKeepaliveEnabled: true,
    gatewayKeepaliveIntervalSecs: 180,
    tokenRefreshPollingEnabled: true,
    tokenRefreshPollIntervalSecs: 60,
    usageRefreshWorkers: 4,
    httpWorkerFactor: 4,
    httpWorkerMin: 8,
    httpStreamWorkerFactor: 1,
    httpStreamWorkerMin: 2,
  },
  envOverrides: {},
  envOverrideCatalog: [],
  envOverrideReservedKeys: [],
  envOverrideUnsupportedKeys: [],
  theme: "tech",
  appearancePreset: "classic",
};

async function mockRuntime(page: import("@playwright/test").Page) {
  await page.route("**/api/runtime**", async (route) => {
    await route.fulfill({
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify({
        mode: "web-gateway",
        rpcBaseUrl: "/api/rpc",
        canManageService: false,
        canSelfUpdate: false,
        canCloseToTray: false,
        canOpenLocalDir: false,
        canUseBrowserFileImport: true,
        canUseBrowserDownloadExport: true,
      }),
    });
  });
}

async function mockApiKeyRpc(
  page: import("@playwright/test").Page,
  options: {
    apiKeys?: unknown[];
    aggregateApis?: unknown[];
    modelRoutes?: Record<string, unknown[]>;
    stripModelRoutes?: boolean;
    onMethod?: (method: string, payload: Record<string, unknown>) => unknown | undefined;
  } = {},
) {
  const apiKeys =
    options.apiKeys ||
    [
      {
        id: "key-spark",
        name: "Spark Key",
        model_slug: "gpt-5.3-codex-unknown",
        reasoning_effort: "medium",
        service_tier: "default",
        protocol_type: "openai_compat",
        rotation_strategy: "account_rotation",
        status: "enabled",
        created_at: 1_770_000_000,
      },
    ];

  await page.route("**/api/rpc**", async (route) => {
    const payload = route.request().postDataJSON() as Record<string, unknown>;
    const method = typeof payload?.method === "string" ? payload.method : "";
    const id = payload?.id ?? 1;

    const ok = (result: unknown) =>
      route.fulfill({
        contentType: "application/json; charset=utf-8",
        body: JSON.stringify({
          jsonrpc: "2.0",
          id,
          result,
        }),
      });

    const customResult = options.onMethod?.(method, payload);
    if (customResult !== undefined) {
      await ok(customResult);
      return;
    }

    if (method === "appSettings/get") {
      await ok(SETTINGS_SNAPSHOT);
      return;
    }
    if (method === "initialize") {
      await ok({
        userAgent: "codex_cli_rs/0.1.19",
        codexHome: "C:/Users/Test/.codex",
        platformFamily: "windows",
        platformOs: "windows",
      });
      return;
    }
    if (method === "accountManager/session/current") {
      await ok({
        mode: "none",
        currentUser: null,
        role: "system_admin",
        permissions: ["system:admin"],
        distributionEnabled: false,
      });
      return;
    }
    if (method === "gateway/concurrencyRecommendation/get") {
      await ok({
        usageRefreshWorkers: 4,
        httpWorkerFactor: 4,
        httpWorkerMin: 8,
        httpStreamWorkerFactor: 1,
        httpStreamWorkerMin: 2,
        accountMaxInflight: 1,
      });
      return;
    }
    if (method === "apikey/list") {
      await ok({ items: apiKeys });
      return;
    }
    if (method === "account/list") {
      await ok({
        items: [
          {
            id: "account-team-a",
            label: "Team A account",
            group_name: "team-a",
            status: "active",
            sort: 0,
          },
          {
            id: "account-team-b",
            label: "Team B account",
            group_name: "team-b",
            status: "active",
            sort: 1,
          },
        ],
        total: 2,
        page: 1,
        pageSize: 2,
      });
      return;
    }
    if (method === "aggregateApi/list") {
      await ok({ items: options.aggregateApis ?? [] });
      return;
    }
    if (method === "apikey/managedModelListV2") {
      const items = [
          {
            id: "builtin:gpt-5.3-codex",
            slug: "gpt-5.3-codex",
            displayName: "GPT-5.3 Codex",
            description: "Latest frontier agentic coding model.",
            provider: "openai",
            family: "gpt-5",
            category: "codex",
            tags: ["reasoning", "coding"],
            origin: "builtin",
            enabled: true,
            supportedInApi: true,
            visibility: "list",
            sortOrder: 0,
            contextWindow: 400000,
            maxContextWindow: 400000,
            defaultReasoningEffort: "medium",
            capabilities: { inputModalities: ["text", "image"] },
            instructionsMode: "fallback",
            instructionsText: null,
            fastPolicy: "passthrough",
            builtinRevision: 1,
            userEdited: false,
            price: {
              priceStatus: "official",
              priceSource: "e2e-fixture",
              inputMicrousdPer1m: 1250000,
              cachedInputMicrousdPer1m: 125000,
              outputMicrousdPer1m: 10000000,
            },
            priceTiers: [],
            routes: [{
              id: "gpt-account-route",
              sourceKind: "account_pool",
              sourceId: "default",
              upstreamModel: "gpt-5.3-codex",
              enabled: true,
              priority: 0,
              weight: 1,
            }],
            permissionGroupIds: [],
            createdAt: 1770000000,
            updatedAt: 1770000000,
          },
          {
            id: "custom:claude-sonnet-test",
            slug: "claude-sonnet-test",
            displayName: "Claude Sonnet Test",
            description: "Test-only Claude text model.",
            provider: "anthropic",
            family: "claude",
            category: "text",
            tags: ["text"],
            origin: "custom",
            enabled: true,
            supportedInApi: true,
            visibility: "list",
            sortOrder: 2,
            contextWindow: 200000,
            maxContextWindow: 200000,
            defaultReasoningEffort: null,
            capabilities: { inputModalities: ["text"] },
            instructionsMode: "passthrough",
            instructionsText: null,
            fastPolicy: "passthrough",
            builtinRevision: null,
            userEdited: false,
            price: {
              priceStatus: "missing",
              priceSource: null,
              inputMicrousdPer1m: null,
              cachedInputMicrousdPer1m: null,
              outputMicrousdPer1m: null,
            },
            priceTiers: [],
            routes: [{
              id: "claude-route-1",
              sourceKind: "aggregate_api",
              sourceId: "claude-upstream-1",
              upstreamModel: "claude-sonnet-test",
              enabled: true,
              priority: 0,
              weight: 1,
            }],
            permissionGroupIds: [],
            createdAt: 1770000000,
            updatedAt: 1770000000,
          },
          {
            id: "custom:claude-other-test",
            slug: "claude-other-test",
            displayName: "Claude Other Test",
            description: "Test-only model on another Claude upstream.",
            provider: "openai",
            family: "claude",
            category: "text",
            tags: ["text"],
            origin: "custom",
            enabled: true,
            supportedInApi: true,
            visibility: "list",
            sortOrder: 3,
            contextWindow: 200000,
            maxContextWindow: 200000,
            defaultReasoningEffort: null,
            capabilities: { inputModalities: ["text"] },
            instructionsMode: "passthrough",
            instructionsText: null,
            fastPolicy: "passthrough",
            builtinRevision: null,
            userEdited: false,
            price: {
              priceStatus: "missing",
              priceSource: null,
              inputMicrousdPer1m: null,
              cachedInputMicrousdPer1m: null,
              outputMicrousdPer1m: null,
            },
            priceTiers: [],
            routes: [{
              id: "claude-route-other",
              sourceKind: "aggregate_api",
              sourceId: "claude-upstream-other",
              upstreamModel: "claude-other-test",
              enabled: true,
              priority: 0,
              weight: 1,
            }],
            permissionGroupIds: [],
            createdAt: 1770000000,
            updatedAt: 1770000000,
          },
          {
            id: "builtin:gpt-image-2",
            slug: "gpt-image-2",
            displayName: "GPT Image 2",
            description: "State-of-the-art image generation and editing model.",
            provider: "openai",
            family: "gpt-image",
            category: "image",
            tags: ["image-generation", "image-editing"],
            origin: "builtin",
            enabled: true,
            supportedInApi: true,
            visibility: "list",
            sortOrder: 44,
            contextWindow: null,
            maxContextWindow: null,
            defaultReasoningEffort: null,
            capabilities: {
              reasoning_efforts: [],
              service_tiers: [],
              additional_speed_tiers: [],
              input_modalities: ["text", "image"],
              output_modalities: ["image"],
              supported_endpoints: [
                "/v1/images/generations",
                "/v1/images/edits",
              ],
              supports_text_generation: false,
            },
            instructionsMode: "passthrough",
            instructionsText: null,
            fastPolicy: "passthrough",
            builtinRevision: 5,
            userEdited: false,
            price: {
              priceStatus: "official",
              priceSource:
                "https://developers.openai.com/api/docs/pricing#image-generation",
              inputMicrousdPer1m: 8000000,
              cachedInputMicrousdPer1m: 2000000,
              outputMicrousdPer1m: 30000000,
            },
            priceTiers: [],
            routes: [],
            permissionGroupIds: [],
            createdAt: 1770000000,
            updatedAt: 1770000000,
          },
        ];
      await ok({
        items: items.map((model) => ({
          ...model,
          routes: options.stripModelRoutes
            ? []
            : options.modelRoutes?.[model.slug] ?? model.routes,
        })),
        stats: {
          total: 4,
          enabled: 4,
          builtin: 2,
          custom: 2,
          priceMissing: 2,
          missingRoute: 2,
        },
      });
      return;
    }
    if (method === "apikey/usageStats") {
      await ok([]);
      return;
    }

    await route.fulfill({
      status: 500,
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify({
        jsonrpc: "2.0",
        id,
        error: {
          code: -32000,
          message: `Unhandled RPC method in test: ${method}`,
        },
      }),
    });
  });
}

test("api key modal reuses prefix model metadata for long model slugs", async ({ page }) => {
  await mockRuntime(page);
  await mockApiKeyRpc(page);

  await page.goto("/apikeys/");
  await expect(page.getByRole("main").getByRole("heading", { name: "平台密钥" })).toBeVisible();
  await expect(page.locator("tr", { hasText: "Spark Key" })).toBeVisible();
  await expect(page.locator("tr", { hasText: "gpt-5.3-codex-unknown" })).toBeVisible();

  await page.locator("tr", { hasText: "Spark Key" }).getByTitle("编辑配置").click();

  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "编辑平台密钥" })).toBeVisible();
  await dialog.getByText("GPT-5.3 Codex", { exact: true }).click();
  await expect(
    page.getByRole("option", { name: /GPT-5\.3 Codex/ }).first(),
  ).toBeVisible();
  await expect(page.getByRole("option", { name: "Claude Other Test" })).toHaveCount(0);
});

test("api key modal hides image-only models when creating a key", async ({ page }) => {
  await mockRuntime(page);
  await mockApiKeyRpc(page, { apiKeys: [] });

  await page.goto("/apikeys/");
  await page.getByRole("button", { name: "创建密钥" }).click();

  const dialog = page.getByRole("dialog");
  await dialog.locator("#api-key-upstream-provider").click();
  await page.getByRole("option", { name: "OpenAI 上游池" }).click();
  const modelSelect = dialog
    .getByText("绑定模型 (可选)", { exact: true })
    .locator("..")
    .getByRole("combobox");
  await modelSelect.click();

  await expect(page.getByRole("option", { name: "GPT-5.3 Codex" })).toBeVisible();
  await expect(page.getByRole("option", { name: "GPT Image 2" })).toHaveCount(0);
});

test("api key modal can migrate an existing image-only binding to a text model", async ({
  page,
}) => {
  const updatePayloads: Record<string, unknown>[] = [];
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [
      {
        id: "key-image",
        name: "Image Key",
        model_slug: "gpt-image-2",
        reasoning_effort: "auto",
        service_tier: "auto",
        protocol_type: "openai_compat",
        rotation_strategy: "account_rotation",
        status: "enabled",
        created_at: 1_770_000_002,
      },
    ],
    onMethod: (method, payload) => {
      if (method === "apikey/updateModel") {
        updatePayloads.push(payload);
        return { ok: true };
      }
      return undefined;
    },
  });

  await page.goto("/apikeys/");
  await page.locator("tr", { hasText: "Image Key" }).getByTitle("编辑配置").click();

  const dialog = page.getByRole("dialog");
  const modelSelect = dialog
    .getByText("绑定模型 (可选)", { exact: true })
    .locator("..")
    .getByRole("combobox");
  await expect(modelSelect).toContainText("GPT Image 2");
  await modelSelect.click();
  await expect(page.getByRole("option", { name: "GPT Image 2" })).toHaveCount(0);
  await page.getByRole("option", { name: "GPT-5.3 Codex" }).click();
  await expect(modelSelect).toContainText("GPT-5.3 Codex");

  await dialog.getByRole("button", { name: "完成" }).click();

  await expect.poll(() => updatePayloads.length).toBe(1);
  const params = updatePayloads[0]?.params as Record<string, unknown>;
  expect(params.modelSlug).toBe("gpt-5.3-codex");
});

test("api key modal displays and submits hybrid rotation", async ({ page }) => {
  const updatePayloads: Record<string, unknown>[] = [];
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [
      {
        id: "key-hybrid",
        name: "Hybrid Key",
        model_slug: "gpt-5.3-codex-unknown",
        reasoning_effort: "medium",
        service_tier: "default",
        protocol_type: "openai_compat",
        rotation_strategy: "hybrid_rotation",
        account_plan_filter: "plus",
        account_group_filter: "team-a",
        status: "enabled",
        created_at: 1_770_000_001,
      },
    ],
    onMethod: (method, payload) => {
      if (method === "apikey/updateModel") {
        updatePayloads.push(payload);
        return { ok: true };
      }
      return undefined;
    },
  });

  await page.goto("/apikeys/");
  const row = page.locator("tr", { hasText: "Hybrid Key" });
  await expect(row).toBeVisible();
  await expect(row.getByText("混合轮转（账号优先）", { exact: true })).toBeVisible();
  await expect(row.getByText("账号分组: team-a", { exact: true })).toBeVisible();

  await row.getByTitle("编辑配置").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "编辑平台密钥" })).toBeVisible();
  await expect(dialog.getByText("混合轮转（账号优先）", { exact: true })).toBeVisible();
  await expect(dialog.getByText("账号计划筛选", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Plus", { exact: true })).toBeVisible();
  await expect(dialog.getByText("账号分组筛选", { exact: true })).toBeVisible();
  await expect(dialog.getByText("team-a", { exact: true })).toBeVisible();

  await dialog.getByRole("button", { name: "完成" }).click();

  await expect.poll(() => updatePayloads.length).toBe(1);
  const params = updatePayloads[0]?.params as Record<string, unknown>;
  expect(params.rotationStrategy).toBe("hybrid_rotation");
  expect(params.accountPlanFilter).toBe("plus");
  expect(params.accountGroupFilter).toBe("team-a");
});

test("api key modal can select hybrid rotation on create", async ({ page }) => {
  const createPayloads: Record<string, unknown>[] = [];
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [],
    onMethod: (method, payload) => {
      if (method === "apikey/create") {
        createPayloads.push(payload);
        return { id: "key-created", key: "cm-test-key" };
      }
      return undefined;
    },
  });

  await page.goto("/apikeys/");
  await page.getByRole("button", { name: "创建密钥" }).click();

  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("heading", { name: "创建平台密钥" })).toBeVisible();
  await dialog.locator("#api-key-upstream-provider").click();
  await page.getByRole("option", { name: "OpenAI 上游池" }).click();
  await expect(dialog.getByLabel("自定义 API Key (可选)")).toBeVisible();
  await dialog.getByLabel("自定义 API Key (可选)").fill("sk-cm-custom-fixed");
  await dialog.getByText("账号轮转", { exact: true }).click();
  await page.getByText("混合轮转（账号优先）", { exact: true }).click();
  await expect(dialog.getByText("账号计划筛选", { exact: true })).toBeVisible();
  await expect(dialog.getByText("账号分组筛选", { exact: true })).toBeVisible();
  await dialog.getByRole("button", { name: "完成" }).click();

  await expect.poll(() => createPayloads.length).toBe(1);
  await expect(dialog).not.toBeVisible();
  const params = createPayloads[0]?.params as Record<string, unknown>;
  expect(params.rotationStrategy).toBe("hybrid_rotation");
  expect(params.upstreamProvider).toBe("openai");
  expect(params.customKey).toBe("sk-cm-custom-fixed");
});

test("admin OpenAI key model choices follow active account and aggregate routes", async ({ page }) => {
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [{
      id: "openai-aggregate-key",
      name: "OpenAI aggregate key",
      upstream_provider: "openai",
      rotation_strategy: "aggregate_api_rotation",
      status: "enabled",
    }],
    aggregateApis: [
      { id: "compatible-upstream", provider_type: "compatible", status: "active" },
      { id: "disabled-codex", provider_type: "codex", status: "disabled" },
      { id: "claude-upstream-other", provider_type: "claude", status: "active" },
    ],
    modelRoutes: {
      "claude-sonnet-test": [{
        sourceKind: "aggregate_api",
        sourceId: "compatible-upstream",
        enabled: true,
      }],
      "claude-other-test": [
        { sourceKind: "aggregate_api", sourceId: "claude-upstream-other", enabled: true },
        { sourceKind: "aggregate_api", sourceId: "disabled-codex", enabled: true },
      ],
    },
  });

  await page.goto("/apikeys/");
  await page.locator("tr", { hasText: "OpenAI aggregate key" }).getByTitle("编辑配置").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("通配兼容 (Codex / Claude Code / Gemini CLI)")).toBeVisible();
  const modelSelect = dialog
    .getByText("绑定模型 (可选)", { exact: true })
    .locator("..")
    .getByRole("combobox");
  await modelSelect.click();
  await expect(page.getByRole("option", { name: "Claude Sonnet Test" })).toBeVisible();
  await expect(page.getByRole("option", { name: "GPT-5.3 Codex" })).toHaveCount(0);
  await expect(page.getByRole("option", { name: "Claude Other Test" })).toHaveCount(0);
});

test("Claude platform key selects the isolated subscription pool and its model routes", async ({ page }) => {
  const createPayloads: Record<string, unknown>[] = [];
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [],
    modelRoutes: {
      "claude-sonnet-test": [{ sourceKind: "aggregate_api", sourceId: "claude-api-key", enabled: true }],
      "claude-other-test": [{ sourceKind: "account_pool", sourceId: "claude", enabled: true }],
    },
    onMethod: (method, payload) => {
      if (method === "apikey/create") {
        createPayloads.push(payload);
        return { id: "key-claude", key: "cm-claude-test-key" };
      }
      return undefined;
    },
  });

  await page.goto("/apikeys/");
  await page.getByRole("button", { name: "创建密钥" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "完成" }).click();
  await expect(page.getByText("操作失败: 请选择上游池")).toBeVisible();
  expect(createPayloads).toHaveLength(0);

  await dialog.locator("#api-key-upstream-provider").click();
  await page.getByRole("option", { name: "Claude 上游" }).click();
  await expect(dialog.getByText("账号轮转", { exact: true })).toBeVisible();
  await expect(dialog.getByText("Claude 订阅账号池（Messages / Responses）")).toBeVisible();
  await expect(dialog.getByText("Claude 订阅池支持 POST /v1/messages、POST /v1/responses、本地估算的 POST /v1/messages/count_tokens 和 GET /v1/models；不支持 POST /v1/chat/completions。")).toBeVisible();
  await expect(dialog.getByText("通配兼容 (Codex / Claude Code / Gemini CLI)")).toHaveCount(0);
  await expect(dialog.getByText("账号计划筛选", { exact: true })).toHaveCount(0);

  const modelSelect = dialog
    .getByText("绑定模型 (可选)", { exact: true })
    .locator("..")
    .getByRole("combobox");
  await modelSelect.click();
  await expect(page.getByRole("option", { name: "GPT-5.3 Codex" })).toHaveCount(0);
  await expect(page.getByRole("option", { name: "Claude Sonnet Test" })).toHaveCount(0);
  await expect(page.getByRole("option", { name: "Claude Other Test" })).toBeVisible();
  await page.getByRole("option", { name: "Claude Other Test" }).click();

  await dialog.getByRole("button", { name: "完成" }).click();
  await expect.poll(() => createPayloads.length).toBe(1);
  const params = createPayloads[0]?.params as Record<string, unknown>;
  expect(params.upstreamProvider).toBe("claude");
  expect(params.rotationStrategy).toBe("account_rotation");
  expect(params.aggregateApiId).toBeNull();
  expect(params.modelSlug).toBe("claude-other-test");
});

test("a migrated key requires an active administrator pool choice before route review clears", async ({ page }) => {
  const updatePayloads: Record<string, unknown>[] = [];
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [{
      id: "key-needs-route-review",
      name: "Legacy ambiguous key",
      upstream_provider: "openai",
      requires_route_review: true,
      rotation_strategy: "aggregate_api_rotation",
      status: "disabled",
    }],
    onMethod: (method, payload) => {
      if (method === "apikey/updateModel") {
        updatePayloads.push(payload);
        return { ok: true };
      }
      return undefined;
    },
  });
  await page.goto("/apikeys/");
  const row = page.locator("tr", { hasText: "Legacy ambiguous key" });
  await expect(row.getByText("需检查路由")).toBeVisible();
  await expect(row.getByRole("switch")).toBeDisabled();
  await row.getByTitle("编辑配置").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("此 Key 升级后需要重新选择上游池，保存后才能启用。"))
    .toBeVisible();
  await dialog.getByRole("button", { name: "完成" }).click();
  await expect(page.getByText("操作失败: 请选择上游池")).toBeVisible();
  expect(updatePayloads).toHaveLength(0);
  await dialog.locator("#api-key-upstream-provider").click();
  await page.getByRole("option", { name: "OpenAI 上游池" }).click();
  await dialog.getByRole("button", { name: "完成" }).click();
  await expect.poll(() => updatePayloads.length).toBe(1);
  const params = updatePayloads[0]?.params as Record<string, unknown>;
  expect(params.upstreamProvider).toBe("openai");
  expect(params.confirmRouteReview).toBe(true);
  expect(params.rotationStrategy).toBe("account_rotation");
});

test("editing a legacy OpenAI key clears cross-provider route settings before selecting Claude", async ({ page }) => {
  const updatePayloads: Record<string, unknown>[] = [];
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [{
      id: "legacy-key",
      name: "Legacy key",
      model_slug: "gpt-5.3-codex",
      rotation_strategy: "hybrid_rotation",
      account_group_filter: "team-a",
      status: "enabled",
    }],
    aggregateApis: [{
      id: "claude-upstream-1",
      provider_type: "claude",
      supplier_name: "Claude Console",
      status: "active",
    }],
    onMethod: (method, payload) => {
      if (method === "apikey/updateModel") {
        updatePayloads.push(payload);
        return { ok: true };
      }
      return undefined;
    },
  });

  await page.goto("/apikeys/");
  const row = page.locator("tr", { hasText: "Legacy key" });
  await expect(row.getByText("OpenAI 上游池", { exact: true })).toBeVisible();
  await row.getByTitle("编辑配置").click();
  const dialog = page.getByRole("dialog");
  await dialog.locator("#api-key-upstream-provider").click();
  await page.getByRole("option", { name: "Claude 上游" }).click();
  await expect(dialog.getByText("账号分组筛选", { exact: true })).toHaveCount(0);
  await dialog.getByRole("button", { name: "完成" }).click();

  await expect.poll(() => updatePayloads.length).toBe(1);
  const params = updatePayloads[0]?.params as Record<string, unknown>;
  expect(params.upstreamProvider).toBe("claude");
  expect(params.rotationStrategy).toBe("account_rotation");
  expect(params.aggregateApiId).toBeNull();
  expect(params.modelSlug).toBeNull();
  expect(params.accountGroupFilter).toBeNull();
});

test("editing an existing Claude API key preserves its aggregate route", async ({ page }) => {
  const updatePayloads: Record<string, unknown>[] = [];
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [{
      id: "legacy-claude-api-key",
      name: "Legacy Claude API",
      model_slug: "claude-sonnet-test",
      upstream_provider: "claude",
      rotation_strategy: "aggregate_api_rotation",
      aggregate_api_id: "claude-upstream-1",
      status: "enabled",
    }],
    aggregateApis: [{
      id: "claude-upstream-1",
      provider_type: "claude",
      supplier_name: "Claude Console",
      status: "active",
    }],
    onMethod: (method, payload) => {
      if (method === "apikey/updateModel") {
        updatePayloads.push(payload);
        return { ok: true };
      }
      return undefined;
    },
  });

  await page.goto("/apikeys/");
  const row = page.locator("tr", { hasText: "Legacy Claude API" });
  await expect(row.getByText("Claude API 池", { exact: true })).toBeVisible();
  await row.getByTitle("编辑配置").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByText("Claude API 聚合（依上游能力）")).toBeVisible();
  await expect(dialog.getByText("Claude API 聚合的可用接口取决于所选上游；请按实际模型路由和上游协议接入。")).toBeVisible();
  await expect(dialog.locator("#api-key-claude-upstream")).toContainText("Claude Console");
  await dialog.getByLabel("密钥名称 (可选)").fill("Legacy Claude API renamed");
  await dialog.getByRole("button", { name: "完成" }).click();

  await expect.poll(() => updatePayloads.length).toBe(1);
  const params = updatePayloads[0]?.params as Record<string, unknown>;
  expect(params.upstreamProvider).toBe("claude");
  expect(params.rotationStrategy).toBe("aggregate_api_rotation");
  expect(params.aggregateApiId).toBe("claude-upstream-1");
  expect(params.modelSlug).toBe("claude-sonnet-test");
});

test("a member can see a Claude key's pool but cannot change its provider route", async ({ page }) => {
  const updatePayloads: Record<string, unknown>[] = [];
  const requestedMethods: string[] = [];
  await mockRuntime(page);
  await mockApiKeyRpc(page, {
    apiKeys: [{
      id: "member-claude-key",
      name: "Member Claude key",
      model_slug: "claude-sonnet-test",
      upstream_provider: "claude",
      aggregate_api_id: "claude-upstream-1",
      rotation_strategy: "aggregate_api_rotation",
      status: "enabled",
    }],
    stripModelRoutes: true,
    onMethod: (method, payload) => {
      requestedMethods.push(method);
      if (method === "accountManager/session/current") {
        return {
          mode: "accounts",
          currentUser: { id: "member-1", username: "member", role: "member" },
          role: "member",
          permissions: ["apikey:self", "models:read"],
          distributionEnabled: false,
        };
      }
      if (method === "apikey/updateModel") {
        updatePayloads.push(payload);
        return { ok: true };
      }
      return undefined;
    },
  });

  await page.goto("/apikeys/");
  const row = page.locator("tr", { hasText: "Member Claude key" });
  await expect(row.getByText("Claude API 池", { exact: true })).toBeVisible();
  await row.getByTitle("编辑配置").click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator("#api-key-upstream-provider")).toBeDisabled();
  await expect(dialog.getByText("Claude 上游", { exact: true })).toBeVisible();
  const modelSelect = dialog
    .getByText("绑定模型 (可选)", { exact: true })
    .locator("..")
    .getByRole("combobox");
  await expect(modelSelect).toBeDisabled();
  await expect(modelSelect).toContainText("Claude Sonnet Test");
  await dialog.getByLabel("密钥名称 (可选)").fill("Member renamed Claude key");
  await dialog.getByRole("button", { name: "完成" }).click();

  await expect.poll(() => updatePayloads.length).toBe(1);
  const params = updatePayloads[0]?.params as Record<string, unknown>;
  expect(params).not.toHaveProperty("upstreamProvider");
  expect(params).not.toHaveProperty("aggregateApiId");
  expect(params).not.toHaveProperty("rotationStrategy");
  expect(params).not.toHaveProperty("hasModelConfig");
  expect(params).not.toHaveProperty("modelSlug");
  expect(params.name).toBe("Member renamed Claude key");
  expect(requestedMethods).not.toContain("aggregateApi/list");
});
