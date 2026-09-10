import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { CDP_PORT, closeBrowser, ensureChrome } from "./chrome.mjs";
import {
  captureScenario,
  captureRealView,
  DEFAULT_BASE,
  DEFAULT_REAL_VIEWS,
  DEFAULT_OUT,
  DEFAULT_SCENARIOS,
  openScenario,
  renderReport,
} from "./capture.mjs";

export async function main(argv = process.argv.slice(2)) {
  const [cmd = "run", ...rest] = argv;
  const args = parseArgs(rest);
  if (cmd === "help" || cmd === "--help") return usage();
  if (!["run", "real", "open", "screenshot"].includes(cmd)) {
    usage();
    process.exit(1);
  }

  const chrome = await ensureChrome();

  try {
    const base = args.base || DEFAULT_BASE;
    const outDir = resolve(args.out || DEFAULT_OUT);
    const scenarios = (args.scenarios
      ? String(args.scenarios).split(",")
      : args._.length
        ? args._
        : DEFAULT_SCENARIOS)
      .map(s => s.trim())
      .filter(Boolean);

    if (cmd === "open") {
      const scenario = scenarios[0] || "recording-healthy";
      const page = await openScenario(base, scenario);
      console.log(JSON.stringify({ scenario, url: page.url, targetId: page.targetId }, null, 2));
      return;
    }

    if (cmd === "real") {
      const views = (args.views
        ? String(args.views).split(",")
        : args._.length
          ? args._
          : DEFAULT_REAL_VIEWS)
        .map(s => s.trim())
        .filter(Boolean);
      await mkdir(outDir, { recursive: true });
      const results = [];
      for (const view of views) {
        results.push(await captureRealView(base, view, outDir));
      }
      const report = renderReport(results);
      const reportPath = resolve(outDir, "report.md");
      await writeFile(reportPath, report, "utf8");
      console.log(report);
      console.log(`\nWrote ${reportPath}`);
      return;
    }

    if (cmd === "screenshot") {
      const scenario = scenarios[0] || "recording-healthy";
      await mkdir(outDir, { recursive: true });
      const result = await captureScenario(base, scenario, outDir);
      console.log(JSON.stringify(result, null, 2));
      return;
    }

    await mkdir(outDir, { recursive: true });
    const results = [];
    for (const scenario of scenarios) {
      results.push(await captureScenario(base, scenario, outDir));
    }
    const report = renderReport(results);
    const reportPath = resolve(outDir, "report.md");
    await writeFile(reportPath, report, "utf8");
    console.log(report);
    console.log(`\nWrote ${reportPath}`);
  } finally {
    if (chrome.launched && cmd !== "open" && args["keep-chrome"] !== true) {
      await closeBrowser().catch(() => undefined);
    }
  }
}

function parseArgs(argv) {
  const args = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const key = a.slice(2);
      const next = argv[i + 1];
      if (!next || next.startsWith("--")) args[key] = true;
      else args[key] = argv[++i];
    } else args._.push(a);
  }
  return args;
}

function usage() {
  console.log(`margins UX CDP harness\n\nUsage:\n  node scripts/ux-cdp.mjs run [--base URL] [--out DIR] [--scenarios a,b] [--keep-chrome]\n  node scripts/ux-cdp.mjs real [--base URL] [--out DIR] [--views home,settings-audio] [--keep-chrome]\n  node scripts/ux-cdp.mjs open <scenario> [--base URL]\n  node scripts/ux-cdp.mjs screenshot <scenario> [--out DIR] [--base URL] [--keep-chrome]\n\nChrome is launched with --remote-debugging-port=${CDP_PORT} if no CDP endpoint is available. Run/screenshot close captured tabs; if this harness launched Chrome, they also close that temporary Chrome unless --keep-chrome is passed. Open intentionally leaves the tab available for inspection.`);
}
