import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import ts from "../node_modules/typescript/lib/typescript.js";

const source = await fs.readFile(
  path.resolve(import.meta.dirname, "../src/lib/app-shell/claude-account-access.ts"),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ES2022, target: ts.ScriptTarget.ES2022 },
}).outputText;
const { canManageClaudeAccounts } = await import(
  `data:text/javascript,${encodeURIComponent(compiled)}`
);

test("Claude account management preserves desktop admin access", () => {
  assert.equal(canManageClaudeAccounts(true, true, "member"), true);
  assert.equal(canManageClaudeAccounts(true, false, null), true);
});

test("web Claude account management waits for an admin session", () => {
  assert.equal(canManageClaudeAccounts(false, true, "system_admin"), false);
  assert.equal(canManageClaudeAccounts(false, false, "system_admin"), true);
  assert.equal(canManageClaudeAccounts(false, false, "admin"), true);
  assert.equal(canManageClaudeAccounts(false, false, "member"), false);
  assert.equal(canManageClaudeAccounts(false, false, null), false);
});
