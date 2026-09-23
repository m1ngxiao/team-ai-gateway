import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import ts from "../node_modules/typescript/lib/typescript.js";

const source = await fs.readFile(
  path.resolve(import.meta.dirname, "../src/lib/claude-subscription-plan.ts"),
  "utf8",
);
const compiled = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ES2022, target: ts.ScriptTarget.ES2022 },
}).outputText;
const { formatClaudeSubscriptionPlan } = await import(
  `data:text/javascript,${encodeURIComponent(compiled)}`
);

test("Claude subscription organization types display as plan names", () => {
  assert.equal(formatClaudeSubscriptionPlan("claude_pro"), "Pro");
  assert.equal(formatClaudeSubscriptionPlan("claude_max_5x"), "Max");
  assert.equal(formatClaudeSubscriptionPlan("CLAUDE-MAX-20X"), "Max");
  assert.equal(formatClaudeSubscriptionPlan("claude_team"), "Team");
  assert.equal(formatClaudeSubscriptionPlan("enterprise"), "Enterprise");
});

test("unknown Claude organization types never appear raw in the UI", () => {
  assert.equal(formatClaudeSubscriptionPlan("claude_internal_private_value"), "未知方案");
  assert.equal(formatClaudeSubscriptionPlan(null), "未知方案");
});
