import { expect, test, type Page } from "@playwright/test";
import { mkdir } from "node:fs/promises";
import { resolve } from "node:path";

// Deterministic, no-model coverage for the fresh-vault live-cue journey
// (Slice D). Each case drives the browser mock backend — no paid model call — and
// asserts the cue rail is never silent: a fresh vault self-heals with an honest
// "getting set up" line, a genuine quiet still acknowledges the mark, and an
// initialized vault produces a visible suggestion.

const runId = process.env.MARGINS_UX_E2E_RUN_ID || "latest";
const shotDir = resolve(process.cwd(), "ux-e2e-runs", runId, "live-cue-states");

async function openRecording(page: Page, scenario: string) {
  await page.goto(`/?scenario=${scenario}`);
  await page.waitForFunction(() => Boolean(window.__marginsMock?.getState?.()));
  await expect(page.locator("#memo-input-new")).toBeVisible();
}

// Type a mark and request a live cue with Cmd+Enter — the same gesture the app
// binds to "quick assist".
async function requestCueForMark(page: Page, text: string) {
  const input = page.locator("#memo-input-new");
  await input.click();
  await input.fill(text);
  await input.press("Meta+Enter");
}

test.beforeAll(async () => {
  await mkdir(shotDir, { recursive: true });
});

test("fresh vault self-heals with an honest warming line, never silence", async ({ page }) => {
  await openRecording(page, "backchannel-warming");
  await requestCueForMark(page, "pricing pushback again");

  const rail = page.locator(".backchannel-assist-rail");
  await expect(rail).toBeVisible();
  await expect(rail.locator(".backchannel-assist-status")).toHaveText(
    "Getting set up — ask again in a moment.",
  );
  // No enzyme/vault jargon leaks to the user.
  const railText = (await rail.innerText()).toLowerCase();
  expect(railText).not.toContain("enzyme");
  expect(railText).not.toContain("vault");
  expect(railText).not.toContain("catalyze");
  expect(railText).not.toContain("index");
  await page.screenshot({ path: resolve(shotDir, "warming.png") });
});

test("genuine quiet still acknowledges the mark was heard", async ({ page }) => {
  await openRecording(page, "backchannel-quiet");
  await requestCueForMark(page, "small talk about the weather");

  const rail = page.locator(".backchannel-assist-rail");
  await expect(rail).toBeVisible();
  await expect(rail.locator(".backchannel-assist-status")).toHaveText(
    "Heard. Nothing worth pulling you out for.",
  );
  await page.screenshot({ path: resolve(shotDir, "quiet.png") });
});

test("exhausted self-heal shows an honest unavailable line, not a warming loop", async ({ page }) => {
  await openRecording(page, "backchannel-unavailable");
  await requestCueForMark(page, "did that land");

  const rail = page.locator(".backchannel-assist-rail");
  await expect(rail).toBeVisible();
  await expect(rail.locator(".backchannel-assist-status")).toHaveText(
    "Live cues are off for now — your marks are still saved.",
  );
  const railText = (await rail.innerText()).toLowerCase();
  expect(railText).not.toContain("enzyme");
  expect(railText).not.toContain("vault");
  await page.screenshot({ path: resolve(shotDir, "unavailable.png") });
});

test("initialized vault produces a visible suggestion", async ({ page }) => {
  await openRecording(page, "recording-healthy");
  await requestCueForMark(page, "they keep asking about proof");

  const rail = page.locator(".backchannel-assist-rail");
  await expect(rail).toBeVisible();
  // The mock emits a real cue for the initialized-vault path; the question text
  // must render (this is the happy path, no model call).
  await expect(rail.locator(".backchannel-assist-question")).toBeVisible();
  await expect(rail.locator(".backchannel-assist-question")).not.toHaveText("");
  await page.screenshot({ path: resolve(shotDir, "suggestion.png") });
});
