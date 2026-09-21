import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import ts from "../node_modules/typescript/lib/typescript.js";

const appsRoot = path.resolve(import.meta.dirname, "..");
const sourcePath = path.join(appsRoot, "src", "lib", "gateway", "endpoints.ts");

async function loadEndpointModule(publicOrigins = {}) {
  const source = await fs.readFile(sourcePath, "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.ES2022,
      target: ts.ScriptTarget.ES2022,
    },
    fileName: sourcePath,
  });

  const tempDir = await fs.mkdtemp(
    path.join(os.tmpdir(), "codexmanager-gateway-endpoints-"),
  );
  const tempFile = path.join(tempDir, "gateway-endpoints.mjs");
  const originsPath = path.join(appsRoot, "src/lib/gateway/public-origins.ts");
  const originsSource = (await fs.readFile(originsPath, "utf8")).replace(
    /process\.env\.(NEXT_PUBLIC_\w+_ORIGIN)/g,
    (_, name) => JSON.stringify(publicOrigins[name]) ?? "undefined",
  );
  await fs.writeFile(path.join(tempDir, "public-origins.mjs"), ts.transpileModule(originsSource, {
    compilerOptions: { module: ts.ModuleKind.ES2022, target: ts.ScriptTarget.ES2022 },
  }).outputText);
  await fs.writeFile(tempFile, compiled.outputText.replace('"./public-origins"', '"./public-origins.mjs"'), "utf8");
  test.after(() => fs.rm(tempDir, { recursive: true, force: true }));
  return import(pathToFileURL(tempFile).href);
}

const endpoints = await loadEndpointModule();

test("resolveGatewayOrigin 在 Web 网关模式下使用公共 API 缺省值而非管理站 origin", () => {
  assert.equal(
    endpoints.resolveGatewayOrigin({
      browserOrigin: "https://manager.example.com/",
      runtimeMode: "web-gateway",
      serviceAddr: "localhost:48760",
    }),
    "http://127.0.0.1:48760",
  );
});

test("构建注入的 API origin 不会被浏览器管理站 origin 或内部服务地址覆盖", async () => {
  const configured = await loadEndpointModule({
    NEXT_PUBLIC_API_ORIGIN: "https://api.gateway.test/",
    NEXT_PUBLIC_ADMIN_ORIGIN: "https://admin.gateway.test",
    NEXT_PUBLIC_STATS_ORIGIN: "https://stats.gateway.test",
  });
  for (const browserOrigin of ["https://admin.gateway.test", "https://other.test", null]) {
    const origin = configured.resolveGatewayOrigin({
      browserOrigin,
      runtimeMode: "web-gateway",
      serviceAddr: "service:48760",
    });
    assert.equal(origin, "https://api.gateway.test");
    assert.equal(configured.buildOpenAiGatewayEndpoint(origin), "https://api.gateway.test/v1");
  }
});

test("resolveGatewayOrigin 在桌面模式下使用服务地址并补齐协议", () => {
  assert.equal(
    endpoints.resolveGatewayOrigin({
      browserOrigin: "https://manager.example.com",
      runtimeMode: "desktop-tauri",
      serviceAddr: "127.0.0.1:49999",
    }),
    "http://127.0.0.1:49999",
  );
});

test("normalizeGatewayServiceOrigin 统一处理 loopback 与路径碎片", () => {
  assert.equal(
    endpoints.normalizeGatewayServiceOrigin("0.0.0.0:48760/v1"),
    "http://localhost:48760",
  );
  assert.equal(
    endpoints.normalizeGatewayServiceOrigin("http://127.0.0.1:48760/proxy"),
    "http://127.0.0.1:48760",
  );
});

test("normalizeGatewayPublicOrigin 仅接受 http/https", () => {
  assert.equal(
    endpoints.normalizeGatewayPublicOrigin("https://cm.example.com/"),
    "https://cm.example.com",
  );
  assert.equal(endpoints.normalizeGatewayPublicOrigin("tauri://localhost"), "");
});

test("buildOpenAiGatewayEndpoint 只追加一次 /v1", () => {
  assert.equal(
    endpoints.buildOpenAiGatewayEndpoint("http://localhost:48760"),
    "http://localhost:48760/v1",
  );
  assert.equal(
    endpoints.buildOpenAiGatewayEndpoint("http://localhost:48760/v1"),
    "http://localhost:48760/v1",
  );
});

test("buildOpenAiGatewayEndpointFromServiceAddr 复用统一地址规范化", () => {
  assert.equal(
    endpoints.buildOpenAiGatewayEndpointFromServiceAddr("0.0.0.0:48760"),
    "http://localhost:48760/v1",
  );
});

test("buildOpenAiGatewayEndpointFromPublicOrigin 过滤非法公开 origin", () => {
  assert.equal(
    endpoints.buildOpenAiGatewayEndpointFromPublicOrigin("https://cm.example.com/path"),
    "https://cm.example.com/path/v1",
  );
  assert.equal(
    endpoints.buildOpenAiGatewayEndpointFromPublicOrigin("tauri://localhost"),
    "",
  );
});

test("buildClaudeCodeGatewayEndpoint 返回 Claude Code 使用的根地址", () => {
  assert.equal(
    endpoints.buildClaudeCodeGatewayEndpoint("http://localhost:48760/v1"),
    "http://localhost:48760",
  );
});

test("buildGeminiGatewayEndpoint 返回 Gemini CLI 使用的根地址", () => {
  assert.equal(
    endpoints.buildGeminiGatewayEndpoint("http://localhost:48760/v1beta"),
    "http://localhost:48760",
  );
  assert.equal(
    endpoints.buildGeminiGatewayEndpoint("http://localhost:48760/v1alpha"),
    "http://localhost:48760",
  );
});
