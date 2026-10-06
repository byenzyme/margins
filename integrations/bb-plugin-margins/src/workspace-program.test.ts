import { afterEach, describe, expect, it } from "vitest";
import { chmod, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { applyWorkspaceProgram, plainSummaries, planWorkspaceProgram, readWorkspaceProgram } from "./workspace-program.js";

const prior = { cli: process.env.MARGINS_CLI_BIN, home: process.env.MARGINS_HOME };
afterEach(() => {
  if (prior.cli === undefined) delete process.env.MARGINS_CLI_BIN;
  else process.env.MARGINS_CLI_BIN = prior.cli;
  if (prior.home === undefined) delete process.env.MARGINS_HOME;
  else process.env.MARGINS_HOME = prior.home;
});

/** A stand-in `margins` that answers like the real CLI (shapes captured from
 * `workspace show|plan|apply --json`, including margins.error.v1 on stderr). */
async function fakeCli(root: string) {
  const cli = join(root, "margins-test-cli");
  await writeFile(cli, `#!/usr/bin/env node
const fs = require('node:fs');
const a = process.argv.slice(2); fs.appendFileSync(${JSON.stringify(join(root, "calls"))}, a.join(' ') + '\\n');
const fail = (code, message, details = null) => { process.stderr.write(JSON.stringify({schema_version:'margins.error.v1', ok:false, error:{code, message, retryable:false, details}}) + '\\n'); process.exit(1); };
if (a[0] === 'capabilities') console.log(JSON.stringify({schema:1, workspace:{setup:true, program: !fs.existsSync(${JSON.stringify(join(root, "old-cli"))})}}));
else if (a.includes('list')) console.log(JSON.stringify({default_workspace:null, workspaces:[{id:'notes', name:'My Notes'}]}));
else if (a.includes('show')) console.log(JSON.stringify({workspace_id:'notes', program_path:'/m/configs/notes.enzyme', revision:'rev-1', program:'workspace "notes" {}\\n'}, null, 2));
else if (a.includes('plan')) {
  const desired = fs.readFileSync(a[a.indexOf('--desired') + 1], 'utf8');
  fs.writeFileSync(${JSON.stringify(join(root, "seen-desired"))}, desired);
  if (desired.includes('lern')) fail('workspace_desired_invalid', 'invalid desired program: 3:3: expected "}"; found "lern"');
  if (desired.includes('Nope')) fail('workspace_desired_invalid', 'folder reading "Nope" does not exist under Home');
  console.log(JSON.stringify({schema_version:'margins.workspace.plan.v2', workspace_id:'notes', base_revision:'rev-1', plan_id:'p',
    actions:[{action:'set_policy', summary:'Attention policy: learn questions from folder:People', before:{entities:[]}, after:{entities:[{'folder:People':{profile:'auto'}}]}}],
    desired_program:desired, desired_sha256: desired === 'same' ? 'rev-1' : 'rev-2', diff:'--- a/notes.enzyme\\n+++ b/notes.enzyme\\n'}));
} else if (a.includes('apply')) {
  if (fs.existsSync(${JSON.stringify(join(root, "moved"))})) fail('workspace_revision_conflict', 'workspace revision conflict: expected rev-1, found rev-9', {expected_revision:'rev-1', actual_revision:'rev-9'});
  console.log(JSON.stringify({schema_version:'margins.workspace.apply.v2', ok:true, workspace_id:'notes', after_revision:'rev-2'}));
} else console.log('{}');
`);
  await chmod(cli, 0o755);
  process.env.MARGINS_CLI_BIN = cli;
  process.env.MARGINS_HOME = join(root, "machine");
}

describe("Plain-language review summaries", () => {
  it("describes readings, exclusions, and sources in the user's terms", () => {
    expect(plainSummaries({ action: "set_policy", summary: "Attention policy: …",
      before: { entities: ["#old", { "folder:Meetings": { profile: "operational" } }], excluded_folders: ["Archive"], excluded_tags: [] },
      after: { entities: [{ "folder:Meetings": { profile: "decisions" } }, { "folder:Projects": { profile: "decisions" } }],
        excluded_folders: ["Templates"], excluded_tags: ["private"] } })).toEqual([
      "Change how Margins learns from the Meetings folder", "Learn from the Projects folder (new)", "Stop learning from notes tagged #old",
      "Leave out the Templates folder", "Stop leaving out the Archive folder", "Leave out notes tagged #private",
    ]);
    expect(plainSummaries({ action: "add_binding", summary: "x", name: "chat" })).toEqual(['Add the source "chat"']);
    expect(plainSummaries({ action: "update_program", summary: "x" })[0]).toContain("see the diff");
    expect(plainSummaries({ action: "set_policy", summary: "Attention policy: reorder", before: { entities: ["#a"] }, after: { entities: ["#a"] } }))
      .toEqual(["Attention policy: reorder"]);
  });
});

describe("Workspace program editing on the project machine", () => {
  it("reads the program, plans editor text without writing, and applies exactly the reviewed plan", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-program-"));
    try {
      await fakeCli(root);
      await expect(readWorkspaceProgram("notes")).resolves.toEqual({
        workspaceId: "notes", workspaceName: "My Notes", programPath: "/m/configs/notes.enzyme", revision: "rev-1", program: 'workspace "notes" {}\n' });
      const text = 'workspace "notes" {\n  learn questions from folder "People"\n}\n';
      const plan = await planWorkspaceProgram(root, "notes", text);
      expect(plan).toMatchObject({ ok: true, workspaceId: "notes", baseRevision: "rev-1", noop: false,
        actions: [{ action: "set_policy", summary: "Learn from the People folder (new)" }] });
      expect(await readFile(join(root, "seen-desired"), "utf8")).toBe(text);
      // Only the plan is kept; the desired text file is removed.
      expect(await readdir(join(root, "program-plans"))).toEqual([`${(plan as { previewId: string }).previewId}.plan.json`]);
      await expect(planWorkspaceProgram(root, "notes", "same")).resolves.toMatchObject({ ok: true, noop: true });
      await expect(applyWorkspaceProgram(root, "notes", (plan as { previewId: string }).previewId)).resolves.toEqual({ ok: true, revision: "rev-2" });
      const calls = await readFile(join(root, "calls"), "utf8");
      expect(calls).toContain("--workspace notes workspace show --text --json");
      expect(calls).toMatch(/--workspace notes workspace plan --desired \S+\.desired\.enzyme --json/);
      expect(calls).toMatch(new RegExp(`--workspace notes workspace apply --plan \\S+${(plan as { previewId: string }).previewId}\\.plan\\.json --json`));
      // A review whose plan was pruned (or already used) comes back typed, not thrown.
      await expect(applyWorkspaceProgram(root, "notes", (plan as { previewId: string }).previewId)).resolves.toMatchObject({
        ok: false, error: { code: "expired", message: expect.stringContaining("review the changes again") } });
      // An older CLI cannot show or plan programs: ask for an update instead of a raw argument error.
      await writeFile(join(root, "old-cli"), "");
      await expect(readWorkspaceProgram("notes")).rejects.toThrow("Update Margins");
    } finally { await rm(root, { recursive: true, force: true }); }
  });

  it("reports parse errors with their location, resolve errors without one, and stale plans as stale", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-program-"));
    try {
      await fakeCli(root);
      await expect(planWorkspaceProgram(root, "notes", "workspace \"notes\" {\n  source\n  lern\n}")).resolves.toEqual({ ok: false, error: {
        code: "workspace_desired_invalid", message: 'invalid desired program: 3:3: expected "}"; found "lern"', line: 3, column: 3 } });
      await expect(planWorkspaceProgram(root, "notes", 'learn questions from folder "Nope"')).resolves.toEqual({ ok: false, error: {
        code: "workspace_desired_invalid", message: 'folder reading "Nope" does not exist under Home', line: null, column: null } });
      const plan = await planWorkspaceProgram(root, "notes", "changed") as { previewId: string };
      await writeFile(join(root, "moved"), "");
      await expect(applyWorkspaceProgram(root, "notes", plan.previewId)).resolves.toMatchObject({ ok: false,
        error: { code: "stale", actualRevision: "rev-9", line: null, column: null } });
      await expect(applyWorkspaceProgram(root, "other", plan.previewId)).rejects.toThrow("different Workspace");
      await expect(applyWorkspaceProgram(root, "notes", "../../etc/passwd")).rejects.toThrow("Invalid");
      await expect(readWorkspaceProgram("../notes")).rejects.toThrow("Invalid");
    } finally { await rm(root, { recursive: true, force: true }); }
  });
});

// Opt-in: MARGINS_PROGRAM_E2E_CLI=<path to a built margins binary>. Runs the
// editor's read → plan → apply → stale path against the real CLI in a temp home.
describe.skipIf(!process.env.MARGINS_PROGRAM_E2E_CLI)("Workspace program editing with the real Margins CLI", () => {
  it("validates, applies, and refuses a plan made stale by another edit", async () => {
    const root = await mkdtemp(join(tmpdir(), "margins-program-e2e-"));
    const { execFile } = await import("node:child_process");
    const { mkdir } = await import("node:fs/promises");
    const run = (args: string[]) => new Promise<string>((resolve, reject) => execFile(process.env.MARGINS_PROGRAM_E2E_CLI!, args,
      { env: { ...process.env, HOME: root, MARGINS_HOME: join(root, "home") } }, (error, stdout) => error ? reject(error) : resolve(stdout)));
    try {
      const notes = join(root, "notes");
      await mkdir(join(notes, "People"), { recursive: true });
      await run(["workspace", "new", "notes", "--home", notes, "--json"]);
      process.env.MARGINS_CLI_BIN = process.env.MARGINS_PROGRAM_E2E_CLI;
      process.env.MARGINS_HOME = join(root, "home");
      const HOME = process.env.HOME;
      process.env.HOME = root;
      try {
        const saved = await readWorkspaceProgram("notes");
        expect(saved.programPath).toBe(join(root, "home", "configs", "notes.enzyme"));
        const broken = saved.program.replace("remember in", "lern questions\n  remember in");
        const parse = await planWorkspaceProgram(root, "notes", broken);
        const brokenLine = broken.split("\n").findIndex((line) => line.includes("lern")) + 1;
        expect(parse).toMatchObject({ ok: false, error: { code: "workspace_desired_invalid", line: brokenLine, column: 3 } });
        const edited = saved.program.replace(/\n}\s*$/, '\n  learn questions from folder "People" about relationships\n}\n');
        const plan = await planWorkspaceProgram(root, "notes", edited);
        expect(plan).toMatchObject({ ok: true, baseRevision: saved.revision, noop: false,
          actions: [{ action: "set_policy", summary: "Learn from the People folder (new)" }] });
        const unchanged = await planWorkspaceProgram(root, "notes", saved.program);
        expect(unchanged).toMatchObject({ ok: true, noop: true });
        // Another editor saves first.
        const other = await planWorkspaceProgram(root, "notes", saved.program.replace(/\n}\s*$/, "\n  question budget 5\n}\n"));
        await expect(applyWorkspaceProgram(root, "notes", (other as { previewId: string }).previewId)).resolves.toMatchObject({ ok: true });
        const stale = await applyWorkspaceProgram(root, "notes", (plan as { previewId: string }).previewId);
        expect(stale).toMatchObject({ ok: false, error: { code: "stale" } });
        expect((await readWorkspaceProgram("notes")).program).toContain("question budget 5");
        // Re-planned against the new saved program, the same text applies.
        const replanned = await planWorkspaceProgram(root, "notes", edited);
        await expect(applyWorkspaceProgram(root, "notes", (replanned as { previewId: string }).previewId)).resolves.toMatchObject({ ok: true });
        expect((await readWorkspaceProgram("notes")).program).toBe(edited);
      } finally { process.env.HOME = HOME; }
    } finally { await rm(root, { recursive: true, force: true }); }
  }, 60_000);
});
