import { defineConfig } from "@playwright/test";

const runId = process.env.MARGINS_UX_E2E_RUN_ID || "latest";

export default defineConfig({
  testDir: ".",
  testMatch: "**/*.spec.ts",
  outputDir: `../../ux-e2e-runs/${runId}/test-results`,
  reporter: [
    ["list"],
    ["html", { outputFolder: `../../ux-e2e-runs/${runId}/playwright-report`, open: "never" }],
  ],
  workers: 1,
  timeout: 180_000,
  projects: [
    {
      name: "chromium",
      use: { browserName: "chromium" },
    },
  ],
  use: {
    baseURL: "http://127.0.0.1:5173",
    viewport: { width: 1280, height: 860 },
    deviceScaleFactor: 2,
    headless: true,
    screenshot: "on",
    trace: "on",
    video: {
      mode: "on",
      size: { width: 1280, height: 860 },
    },
  },
  webServer: {
    command: "npm run dev -- --host 127.0.0.1",
    url: "http://127.0.0.1:5173",
    reuseExistingServer: true,
    timeout: 120_000,
  },
});
