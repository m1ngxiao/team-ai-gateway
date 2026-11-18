import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const testDir = path.dirname(fileURLToPath(import.meta.url));
const appFramePath = path.join(
  testDir,
  "..",
  "src",
  "components",
  "layout",
  "app-frame.tsx",
);

test("主内容区使用真实桌面坐标系，不再通过缩放压缩界面", async () => {
  const source = await fs.readFile(appFramePath, "utf8");

  assert.match(source, /data-slot="app-main-scale"/);
  assert.doesNotMatch(source, /xl:scale-90/);
  assert.doesNotMatch(source, /111\.111111%/);
  assert.match(source, /flex h-full w-full flex-col/);
  assert.match(source, /px-4 pb-10 pt-5/);
  assert.match(source, /lg:px-7 lg:pt-6/);
  assert.match(source, /xl:px-8/);
});
