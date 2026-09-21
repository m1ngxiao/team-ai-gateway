import assert from "node:assert/strict";
import fs from "node:fs/promises";
import test from "node:test";
import ts from "../node_modules/typescript/lib/typescript.js";

const source = await fs.readFile(new URL("../src/lib/gateway/public-origins.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ES2022, target: ts.ScriptTarget.ES2022 },
}).outputText;
const { resolvePublicOrigins, validatePublicOrigin } = await import(
  `data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`
);

test("public origins have separate loopback defaults for API, administration, and statistics", () => {
  assert.deepEqual(resolvePublicOrigins({}), {
    api: "http://127.0.0.1:48760",
    admin: "http://127.0.0.1:48761",
    stats: "http://127.0.0.1:48763",
  });
});

test("public origins normalize HTTPS and permit HTTP only for loopback", () => {
  for (const [input, expected] of [
    [" https://API.gateway.test:443/ ", "https://api.gateway.test"],
    ["https://api.gateway.test:8443", "https://api.gateway.test:8443"],
    ["http://localhost:48760/", "http://localhost:48760"],
    ["http://127.10.20.30:48760", "http://127.10.20.30:48760"],
    ["http://[::1]:48760", "http://[::1]:48760"],
  ]) assert.equal(validatePublicOrigin(input, "NEXT_PUBLIC_API_ORIGIN"), expected);
});

test("every explicit invalid public origin fails instead of falling back or stripping unsafe parts", () => {
  const invalidOrigins = [
    "", " ", "api.gateway.test", "ftp://api.gateway.test", "http://api.gateway.test",
    "http://10.0.0.1:48760", "http://0.0.0.0:48760", "http://[::]:48760",
    "https://user:secret@api.gateway.test", "https://@api.gateway.test",
    "https://api.gateway.test/v1", "https://api.gateway.test//",
    "https://api.gateway.test/..", "https://api.gateway.test/%2e",
    "https://api.gateway.test?", "https://api.gateway.test?token=secret",
    "https://api.gateway.test#", "https://api.gateway.test#fragment",
    "https://api.gateway.test:99999", "https://api.gateway.test\\path",
  ];
  for (const name of ["NEXT_PUBLIC_API_ORIGIN", "NEXT_PUBLIC_ADMIN_ORIGIN", "NEXT_PUBLIC_STATS_ORIGIN"]) {
    for (const origin of invalidOrigins) {
      assert.throws(() => resolvePublicOrigins({ [name]: origin }), (error) => {
        assert.ok(error.message.startsWith(name));
        assert.ok(!error.message.includes("secret"));
        return true;
      }, `${name}: ${origin}`);
    }
  }
});
