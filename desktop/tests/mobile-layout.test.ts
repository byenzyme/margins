import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const styles = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

test("the app shell follows the dynamic mobile viewport", () => {
  assert.match(styles, /body\s*\{[\s\S]*?height:\s*100vh;[\s\S]*?height:\s*100dvh;/);
  assert.match(styles, /#app\s*\{[\s\S]*?height:\s*100vh;[\s\S]*?height:\s*100dvh;/);
});

test("the phone home screen owns a vertical scroller", () => {
  const phoneLayout = styles.match(/@media \(max-width: 720px\) \{[\s\S]*?\/\* --------------------------------------------------------------------------\n   2\. Touch/);
  assert.ok(phoneLayout, "phone layout breakpoint should exist");
  assert.match(phoneLayout[0], /\.workspace-main:has\(\.home-intro-panel\)\s*\{[\s\S]*?align-items:\s*start;/);
  assert.match(phoneLayout[0], /\.workspace-main:has\(\.home-intro-panel\)\s*\{[\s\S]*?overflow-y:\s*auto;/);
  assert.match(phoneLayout[0], /padding:\s*calc\(var\(--window-chrome-height\) \+ 16px\)/);
  assert.match(phoneLayout[0], /\.home-empty-actions\s*\{[\s\S]*?top:\s*2px;[\s\S]*?z-index:\s*21;/);
});

test("the phone sidebar starts closed and closes when a desktop window narrows", () => {
  assert.match(main, /let sidebarCollapsed = isMobileNavigationViewport\(\)/);
  assert.match(main, /mobileNavigationQuery\?\.addEventListener\("change",[\s\S]*?if \(event\.matches\) sidebarCollapsed = true;[\s\S]*?render\(\)/);
});
