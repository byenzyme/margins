import { afterEach, describe, expect, it } from "vitest";
import { chmod, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { applyWorkspaceSetup, previewWorkspaceSetup } from "./workspace-setup.js";

const prior = { cli: process.env.MARGINS_CLI_BIN, home: process.env.MARGINS_HOME };
afterEach(() => {
  if (prior.cli === undefined) delete process.env.MARGINS_CLI_BIN;
  else process.env.MARGINS_CLI_BIN = prior.cli;
  if (prior.home === undefined) delete process.env.MARGINS_HOME;
  else process.env.MARGINS_HOME = prior.home;
});

describe("Workspace setup", () => {
  it("discovers an Obsidian project, compiles a proposal, and applies the exact reviewed plan", async () => {
    // Canonical root: macOS tmpdir() is behind the /var -> /private/var symlink, and setup reports realpaths.
    const root = await realpath(await mkdtemp(join(tmpdir(), "margins-setup-")));
    try {
      const vault = join(root, "notes");
      await mkdir(join(vault, ".obsidian"), { recursive: true });
      const cli = join(root, "margins-test-cli");
      await writeFile(cli, `#!/usr/bin/env node
const fs = require('node:fs');
const a = process.argv.slice(2); fs.appendFileSync(${JSON.stringify(join(root, "calls"))}, a.join(' ') + '\\n');
if (a.includes('list')) console.log(JSON.stringify({workspaces:[]}));
else if (a.includes('compile')) console.log(JSON.stringify({schema_version:'margins.workspace.compile.v1', desired_toml:'id = "notes"\\n', mode:'jev', warning:null, files_scanned:3, selected_entities:['folder:people']}));
else if (a.includes('plan')) console.log(JSON.stringify({workspace_id:'notes', actions:[{kind:'set_policy'}], plan_id:'reviewed'}));
else if (a.includes('destination')) console.log(JSON.stringify({destination:${JSON.stringify(join(vault, "inbox"))}}));
else console.log('{}');
`);
      await chmod(cli, 0o755);
      process.env.MARGINS_CLI_BIN = cli;
      process.env.MARGINS_HOME = join(root, "machine");
      const target = { projectId: "project", projectRoot: vault, hostId: "host", workspaceId: "" };
      const preview = await previewWorkspaceSetup(target, root, "", "inbox");
      expect(await readFile(join(process.env.MARGINS_HOME!, "pending-workspace-setup", "notes"), "utf8")).toBe(vault);
      expect(preview).toMatchObject({ workspaceId: "notes", homeRoot: vault,
        destination: join(vault, "inbox"), filesScanned: 3, selectedEntities: ["folder:people"] });
      expect(JSON.parse(await readFile(join(root, "setup-plans", `${preview.previewId}.plan.json`), "utf8"))).toMatchObject({ plan_id: "reviewed" });
      await expect(applyWorkspaceSetup(root, preview.previewId)).resolves.toEqual({ workspaceId: "notes", destination: join(vault, "inbox") });
      await expect(readFile(join(process.env.MARGINS_HOME!, "pending-workspace-setup", "notes"), "utf8")).rejects.toMatchObject({ code: "ENOENT" });
      const calls = await readFile(join(root, "calls"), "utf8");
      expect(calls.indexOf("workspace apply")).toBeLessThan(calls.indexOf("workspace default --set notes"));
      await expect(previewWorkspaceSetup(target, root, vault, "../other")).rejects.toThrow("relative to Home");
    } finally { await rm(root, { recursive: true, force: true }); }
  });
});
