import { expect, test } from "@playwright/test";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { cleanupRuntimeVault, setupRuntimeVault } from "./runtime-vault.mjs";

const runId = process.env.MARGINS_UX_E2E_RUN_ID || "latest";
const runDir = resolve(process.cwd(), "ux-e2e-runs", runId);
const screenshotDir = resolve(runDir, "screenshots");
const textDir = resolve(runDir, "visible-text");
const fixture = "customer-call";

test.beforeAll(async () => {
  if (process.env.MARGINS_UX_E2E_REUSE_RUNTIME !== "1") {
    await setupRuntimeVault({ runId, fixture });
  }
});

test.afterAll(async () => {
  if (process.env.MARGINS_UX_E2E_REUSE_RUNTIME !== "1") {
    await cleanupRuntimeVault({ runId });
  }
});

test("customer-call-connected-note writes a note with video artifacts", async ({ page }, testInfo) => {
  const timing: Record<string, number | string> = {
    run_id: runId,
    job: "customer-call-connected-note",
    fixture,
    scenario: "ux-runtime-vault",
  };
  await mkdir(screenshotDir, { recursive: true });
  await mkdir(textDir, { recursive: true });

  const startedAt = Date.now();
  const video = page.video();
  let noteSource = "mock-or-unknown";

  await test.step("open finished capture", async () => {
    await page.goto(`/?scenario=ux-runtime-vault&runId=${encodeURIComponent(runId)}`);
    await page.waitForFunction(() => Boolean(window.__marginsMock?.getState?.()));
    await expect(page.getByText("Marks", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Write note" })).toBeVisible();
    await page.screenshot({ path: resolve(screenshotDir, "01-ready-for-note.png") });
    await writeFile(resolve(textDir, "01-ready-for-note.txt"), await page.locator("body").innerText(), "utf8");
    timing.ready_ms = Date.now() - startedAt;
    await page.waitForTimeout(700);
  });

  await test.step("write note", async () => {
    const clickAt = Date.now();
    await page.getByRole("button", { name: "Write note" }).click();
    await expect(page.getByText("Writing note").first()).toBeVisible();
    timing.click_to_writing_note_ms = Date.now() - clickAt;

    await expect(page.getByText(/marks (?:not yet woven in|waiting|awaiting)/i)).toBeVisible();
    await page.screenshot({ path: resolve(screenshotDir, "02-writing-note.png") });
    await writeFile(resolve(textDir, "02-writing-note.txt"), await page.locator("body").innerText(), "utf8");

    await page.waitForFunction(() => /Reading your notes|Finding related ideas|Save note/i.test(document.body.innerText), null, { timeout: 30_000 });
    timing.click_to_first_tool_event_ms = Date.now() - clickAt;

    await page.waitForFunction(() =>
      Array.from(document.querySelectorAll(".grounded-block"))
        .some(el => Boolean(el.textContent?.trim())),
    );
    timing.click_to_first_note_token_ms = Date.now() - clickAt;

    await expect(page.getByText(/Saved to .*customer-call\.md/i)).toBeVisible({ timeout: 90_000 });
    timing.click_to_note_saved_ms = Date.now() - clickAt;
    timing.first_note_token_to_complete_ms = Number(timing.click_to_note_saved_ms) - Number(timing.click_to_first_note_token_ms);
  });

  await test.step("capture saved note", async () => {
    await expect(page.getByText("Refine this note")).toBeVisible();
    await page.screenshot({ path: resolve(screenshotDir, "03-note-saved.png") });
    const bodyText = await page.locator("body").innerText();
    const generatedNote = await page.evaluate(() => window.__marginsMock?.getState?.()?.notes?.["customer-call"] ?? "");
    noteSource = await page.evaluate(() => window.__marginsMock?.getState?.()?.real_distill ? "real" : "mock-or-unknown");
    const distillTrace = await page.evaluate(async () => {
      const mod = await import("/test-harness/mock-tauri.ts");
      return mod.getDistillTrace("customer-call");
    });
    await writeFile(resolve(textDir, "03-note-saved.txt"), bodyText, "utf8");
    await writeFile(resolve(runDir, "generated-note.md"), generatedNote, "utf8");
    await writeFile(resolve(runDir, "distill-trace.json"), `${JSON.stringify(distillTrace, null, 2)}\n`, "utf8");
    timing.total_ms = Date.now() - startedAt;
    await writeFile(resolve(runDir, "timing.json"), `${JSON.stringify(timing, null, 2)}\n`, "utf8");
  });

  await test.step("refine same note", async () => {
    const refineText = "Make the action items sharper, preserve the open questions, and keep the note concise enough to review in two minutes.";
    const refineAt = Date.now();
    await page.getByPlaceholder(/Ask for a change/i).fill(refineText);
    await page.getByRole("button", { name: /Apply change/i }).click();
    await expect(page.getByText(refineText)).toBeVisible();
    await expect(page.getByText("Updated the note with that change.")).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText("Refine this note")).toBeVisible();
    await expect(page.getByText(/Saved to .*customer-call\.md/i)).toBeVisible();
    timing.refine_click_to_first_note_token_ms = Date.now() - refineAt;
    timing.refine_click_to_complete_ms = Date.now() - refineAt;
    await page.screenshot({ path: resolve(screenshotDir, "04-refined-note.png") });
    await writeFile(resolve(textDir, "04-refined-note.txt"), await page.locator("body").innerText(), "utf8");
    await writeFile(resolve(runDir, "timing.json"), `${JSON.stringify(timing, null, 2)}\n`, "utf8");
  });

  await page.close();
  if (video) {
    await video.saveAs(resolve(runDir, "journey.webm"));
    await video.saveAs(testInfo.outputPath("journey.webm"));
  }

  await writeFile(resolve(runDir, "run-manifest.json"), `${JSON.stringify({
    run_id: runId,
    job: "customer-call-connected-note",
    fixture,
    scenario: "ux-runtime-vault",
    runtime_vault_manifest: "runtime-vault-manifest.json",
    runtime_vault_cleanup: "test afterAll removes runtime-vault and runtime JSON",
    note_source: noteSource,
    judge_verdict: "judge-verdict.json",
    raw_video: "journey.webm",
    mp4_video: "journey.mp4",
    test_output_dir: testInfo.outputDir,
  }, null, 2)}\n`, "utf8");
});
