import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";

const appsRoot = path.resolve(import.meta.dirname, "..");

test("management day ranges use the fixed Asia/Shanghai business timezone", async () => {
  const [timeSource, hookSource, apiKeysSource] = await Promise.all([
    fs.readFile(path.join(appsRoot, "src/lib/utils/time.ts"), "utf8"),
    fs.readFile(path.join(appsRoot, "src/hooks/useBusinessDayRange.ts"), "utf8"),
    fs.readFile(path.join(appsRoot, "src/app/apikeys/page.tsx"), "utf8"),
  ]);

  assert.match(timeSource, /BUSINESS_TIME_ZONE = "Asia\/Shanghai"/);
  assert.match(timeSource, /BUSINESS_TIME_ZONE_OFFSET_MS = 8 \* 60 \* 60 \* 1000/);
  assert.match(timeSource, /timeZone: BUSINESS_TIME_ZONE/);
  assert.match(hookSource, /getBusinessDayRange/);
  assert.match(hookSource, /Asia\/Shanghai/);
  assert.match(apiKeysSource, /businessDayRange\.dayStartTs/);
});
