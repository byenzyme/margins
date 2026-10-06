import { afterEach, describe, expect, it } from "vitest";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
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
  it("discovers an Obsidian project, plans the meetings preset, and applies the exact reviewed plan", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-setup-"));
    try {
      const vault = join(root, "notes");
      await mkdir(join(vault, ".obsidian"), { recursive: true });
      const program = 'workspace "notes" {\n  source markdown "home" { path "/notes" }\n\n  learn questions from folder "People"\n\n  remember in folder "Meetings" create note\n}\n';
      const programPath = join(root, "machine", "configs", "notes.enzyme");
      const cli = join(root, "margins-test-cli");
      await writeFile(cli, `#!/usr/bin/env node
const fs = require('node:fs');
const a = process.argv.slice(2); fs.appendFileSync(${JSON.stringify(join(root, "calls"))}, a.join(' ') + '\\n');
if (a.includes('capabilities')) console.log(JSON.stringify({workspace:{setup:true, preset:!process.env.OLD_MARGINS}}));
else if (a.includes('list')) console.log(JSON.stringify({workspaces:[]}));
else if (a.includes('plan')) console.log(JSON.stringify({schema_version:'margins.workspace.plan.v2', workspace_id:'notes', actions:[{action:'set_policy', summary:'Attention policy: learn questions from folder:People'}], plan_id:'reviewed', desired_program:${JSON.stringify(program)}, program_path:${JSON.stringify(programPath)}, preset:{template:'/t', readings:['folder:People'], skipped_readings:['folder:Meetings','folder:Projects'], note_folder:'Meetings'}}));
else if (a.includes('destination')) console.log(JSON.stringify({destination:${JSON.stringify(join(vault, "Meetings"))}}));
else console.log('{}');
`);
      await chmod(cli, 0o755);
      process.env.MARGINS_CLI_BIN = cli;
      process.env.MARGINS_HOME = join(root, "machine");
      const target = { projectId: "project", projectRoot: vault, hostId: "host", workspaceId: "" };
      const preview = await previewWorkspaceSetup(target, root, "");
      expect(await readFile(join(process.env.MARGINS_HOME!, "pending-workspace-setup", "notes"), "utf8")).toBe(vault);
      expect(preview).toMatchObject({ workspaceId: "notes", homeRoot: vault, destination: join(vault, "Meetings"),
        programPath, readings: ["folder:People"], skippedReadings: ["folder:Meetings", "folder:Projects"] });
      expect(JSON.parse(await readFile(join(root, "setup-plans", `${preview.previewId}.plan.json`), "utf8"))).toMatchObject({ plan_id: "reviewed" });
      await expect(applyWorkspaceSetup(root, preview.previewId)).resolves.toEqual({ workspaceId: "notes", destination: join(vault, "Meetings") });
      await expect(readFile(join(process.env.MARGINS_HOME!, "pending-workspace-setup", "notes"), "utf8")).rejects.toMatchObject({ code: "ENOENT" });
      const calls = await readFile(join(root, "calls"), "utf8");
      expect(calls).toContain("--workspace notes workspace plan --preset margins-meetings --json");
      expect(calls).not.toMatch(/compile|scan|--desired/);
      expect(calls.indexOf("workspace apply")).toBeLessThan(calls.indexOf("workspace default --set notes"));
      await expect(previewWorkspaceSetup(target, root, "relative/notes")).rejects.toThrow("absolute notes folder");
      process.env.OLD_MARGINS = "1";
      try {
        await expect(previewWorkspaceSetup(target, root, "")).rejects.toThrow("Update Margins");
      } finally { delete process.env.OLD_MARGINS; }
    } finally { await rm(root, { recursive: true, force: true }); }
  });
});
