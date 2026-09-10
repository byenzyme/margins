import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const mainSource = readFileSync(join(here, "../src/main.ts"), "utf8");
const tauriSource = readFileSync(join(here, "../src/lib/tauri.ts"), "utf8");

test("desktop setup prompt is the minimal CLI-guide handoff", () => {
  assert.match(
    mainSource,
    /"Paste into your agent:",\s*`Set up Margins in \$\{project\.path\}\. Run margins guide workspace-setup and follow it end to end\.`/
  );
  assert.doesNotMatch(mainSource, /\.margins\/skills\/margins-workspace-setup/);
  assert.doesNotMatch(mainSource, /installWorkspaceSkills\(project\.path\)/);
});

test("desktop project registration types do not expose a local setup skill path", () => {
  assert.doesNotMatch(tauriSource, /local_skill_path/);
  assert.doesNotMatch(tauriSource, /has_local_setup_skill/);
});
