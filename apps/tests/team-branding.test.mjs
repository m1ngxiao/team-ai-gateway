import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import ts from "../node_modules/typescript/lib/typescript.js";

const appsRoot = path.resolve(import.meta.dirname, "..");
const sourceRoot = path.join(appsRoot, "src", "lib");
const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "team-relay-i18n-"));

async function compileModule(relativePath) {
  const sourcePath = path.join(sourceRoot, relativePath);
  const source = await fs.readFile(sourcePath, "utf8");
  const compiled = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ES2022, target: ts.ScriptTarget.ES2022 },
    fileName: sourcePath,
  }).outputText.replace(/(from\s+["'])(\.[^"']+)(["'])/g, (_all, before, module, after) => {
    // TypeScript imports use directory shorthand for message catalogs.
    const suffix = module === "./messages" ? "/index.mjs" : ".mjs";
    return `${before}${module}${suffix}${after}`;
  });
  const outputPath = path.join(tempRoot, relativePath.replace(/\.ts$/, ".mjs"));
  await fs.mkdir(path.dirname(outputPath), { recursive: true });
  await fs.writeFile(outputPath, compiled);
}

await compileModule("branding.ts");
await compileModule("i18n/config.ts");
async function compileCatalogs(relativeDir) {
  for (const entry of await fs.readdir(path.join(sourceRoot, relativeDir), { withFileTypes: true })) {
    const relativePath = path.join(relativeDir, entry.name);
    if (entry.isDirectory()) await compileCatalogs(relativePath);
    else if (entry.name.endsWith(".ts")) await compileModule(relativePath);
  }
}
await compileCatalogs("i18n/messages");
const { translate } = await import(pathToFileURL(path.join(tempRoot, "i18n/messages/index.mjs")).href);

test("team display name is used for every locale while user-supplied paths stay unchanged", () => {
  for (const locale of ["zh-CN", "en", "ko", "ru"]) {
    const title = translate(locale, "关于 CodexManager");
    assert.ok(title.includes("团队中转站"), `${locale}: ${title}`);
    assert.ok(!title.includes("CodexManager"), `${locale}: ${title}`);
    const value = "C:/CodexManager/config.toml";
    assert.equal(translate(locale, "CodexManager: {path}", { path: value }), `团队中转站: ${value}`);
  }
});

test("team service information has localized copy without personal-author language", () => {
  for (const locale of ["en", "ko", "ru"]) {
    const message = "遇到接入或用量问题时，请联系团队管理员。";
    const text = translate(locale, message);
    assert.notEqual(text, message, locale);
    assert.ok(!/作者|ProsperGao|赞助/.test(text), `${locale}: ${text}`);
  }
});
