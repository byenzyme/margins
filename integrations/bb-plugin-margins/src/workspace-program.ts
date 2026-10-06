import { randomUUID } from "node:crypto";
import { execFile as execFileCallback } from "node:child_process";
import { mkdir, readFile, readdir, stat, unlink, writeFile } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import { promisify } from "node:util";
import { marginsCli, marginsHome } from "./project-server.js";
import { programErrorLocation } from "./enzyme-highlight.js";
import type { ProgramApplyResult, ProgramError, ProgramPlanResult, WorkspaceProgram } from "./contracts.js";

const execFile = promisify(execFileCallback);
const workspaceIdPattern = /^[a-z0-9][a-z0-9-]*$/;
const previewIdPattern = /^[0-9a-f]{8}-[0-9a-f-]{27}$/;
const PLAN_TTL_MS = 60 * 60 * 1000;
const MAX_KEPT_PLANS = 40;

class CliFailure extends Error {
  constructor(readonly code: string, message: string, readonly details: Record<string, unknown> | null) { super(message); }
}

async function cli(args: string[], timeout = 30_000) {
  try {
    const { stdout } = await execFile(marginsCli(), args, {
      env: { ...process.env, MARGINS_HOME: marginsHome() }, timeout, maxBuffer: 4_000_000,
    });
    return stdout;
  } catch (cause) {
    // With --json the CLI reports failures as margins.error.v1 on stderr.
    const stderr = String((cause as { stderr?: unknown }).stderr || "");
    for (const line of stderr.split("\n").reverse()) {
      try {
        const parsed = JSON.parse(line) as { ok?: boolean; error?: { code?: unknown; message?: unknown; details?: unknown } };
        if (parsed.ok === false && typeof parsed.error?.code === "string" && typeof parsed.error.message === "string") {
          const details = parsed.error.details && typeof parsed.error.details === "object" ? parsed.error.details as Record<string, unknown> : null;
          throw new CliFailure(parsed.error.code, parsed.error.message, details);
        }
      } catch (error) { if (error instanceof CliFailure) throw error; }
    }
    throw cause;
  }
}

function requireWorkspaceId(workspaceId: string) {
  if (!workspaceIdPattern.test(workspaceId)) throw new Error("Invalid Margins Workspace id.");
}

function plansDir(dataDir: string) { return join(dataDir, "program-plans"); }

/** Plans are made on every pause in typing; keep only the recent ones. */
async function prunePlans(dir: string) {
  const entries = await readdir(dir).catch(() => [] as string[]);
  const plans = (await Promise.all(entries.filter((name) => name.endsWith(".plan.json")).map(async (name) => {
    const info = await stat(join(dir, name)).catch(() => null);
    return info ? { name, mtime: info.mtimeMs } : null;
  }))).filter((item): item is { name: string; mtime: number } => item !== null).sort((a, b) => b.mtime - a.mtime);
  const now = Date.now();
  await Promise.all(plans.filter((item, index) => index >= MAX_KEPT_PLANS || now - item.mtime > PLAN_TTL_MS)
    .map((item) => unlink(join(dir, item.name)).catch(() => undefined)));
}

function programError(error: CliFailure): ProgramError {
  const location = programErrorLocation(error.message);
  return { code: error.code, message: error.message, line: location?.line ?? null, column: location?.column ?? null };
}

export async function readWorkspaceProgram(workspaceId: string): Promise<WorkspaceProgram> {
  requireWorkspaceId(workspaceId);
  const shown = JSON.parse(await cli(["--workspace", workspaceId, "workspace", "show", "--text", "--json"])) as {
    workspace_id?: unknown; program_path?: unknown; revision?: unknown; program?: unknown;
  };
  if (shown.workspace_id !== workspaceId || typeof shown.program_path !== "string" || !isAbsolute(shown.program_path)
    || typeof shown.revision !== "string" || typeof shown.program !== "string") {
    throw new Error("Margins returned an invalid Workspace program.");
  }
  return { workspaceId, programPath: shown.program_path, revision: shown.revision, program: shown.program };
}

/** Plan the editor text as the complete desired program. Nothing in the
 * Workspace changes; the plan is kept so apply can commit exactly it. */
export async function planWorkspaceProgram(dataDir: string, workspaceId: string, program: string): Promise<ProgramPlanResult> {
  requireWorkspaceId(workspaceId);
  const dir = plansDir(dataDir);
  await mkdir(dir, { recursive: true, mode: 0o700 });
  await prunePlans(dir);
  const previewId = randomUUID();
  const desired = join(dir, `${previewId}.desired.enzyme`);
  await writeFile(desired, program, { mode: 0o600, flag: "wx" });
  let planJson: string;
  try {
    planJson = await cli(["--workspace", workspaceId, "workspace", "plan", "--desired", desired, "--json"], 60_000);
  } catch (error) {
    if (error instanceof CliFailure) return { ok: false, error: programError(error) };
    throw error;
  } finally { await unlink(desired).catch(() => undefined); }
  const plan = JSON.parse(planJson) as {
    schema_version?: unknown; workspace_id?: unknown; base_revision?: unknown; desired_sha256?: unknown;
    actions?: unknown; diff?: unknown;
  };
  const actions = Array.isArray(plan.actions) ? plan.actions.map((item) => {
    const action = item as { action?: unknown; summary?: unknown };
    return typeof action.action === "string" && typeof action.summary === "string"
      ? { action: action.action, summary: action.summary } : null;
  }) : null;
  if (plan.schema_version !== "margins.workspace.plan.v2" || plan.workspace_id !== workspaceId
    || typeof plan.base_revision !== "string" || typeof plan.desired_sha256 !== "string"
    || typeof plan.diff !== "string" || !actions || actions.some((item) => item === null)) {
    throw new Error("Margins returned an invalid Workspace plan.");
  }
  await writeFile(join(dir, `${previewId}.plan.json`), planJson, { mode: 0o600, flag: "wx" });
  return {
    ok: true, previewId, workspaceId, baseRevision: plan.base_revision,
    noop: plan.base_revision === plan.desired_sha256,
    actions: actions as Array<{ action: string; summary: string }>, diff: plan.diff,
  };
}

/** Commit exactly a plan made by `planWorkspaceProgram`. Margins refuses it
 * when the program changed after planning; that refusal is reported as
 * `stale` so the editor can offer a reload without losing the user's text. */
export async function applyWorkspaceProgram(dataDir: string, workspaceId: string, previewId: string): Promise<ProgramApplyResult> {
  requireWorkspaceId(workspaceId);
  if (!previewIdPattern.test(previewId)) throw new Error("Invalid Workspace program plan.");
  const planFile = join(plansDir(dataDir), `${previewId}.plan.json`);
  const plan = JSON.parse(await readFile(planFile, "utf8").catch(() => {
    throw new Error("This review expired. Review the changes again.");
  })) as { workspace_id?: unknown };
  if (plan.workspace_id !== workspaceId) throw new Error("The reviewed plan is for a different Workspace.");
  let receipt: { ok?: unknown; after_revision?: unknown };
  try {
    receipt = JSON.parse(await cli(["--workspace", workspaceId, "workspace", "apply", "--plan", planFile, "--json"]));
  } catch (error) {
    if (!(error instanceof CliFailure)) throw error;
    if (error.code === "workspace_revision_conflict") {
      const actual = error.details?.actual_revision;
      return { ok: false, error: { code: "stale", message: "The program changed outside this editor after you reviewed it.",
        line: null, column: null, ...(typeof actual === "string" ? { actualRevision: actual } : {}) } };
    }
    return { ok: false, error: programError(error) };
  }
  if (receipt.ok !== true || typeof receipt.after_revision !== "string") throw new Error("Margins returned an invalid apply receipt.");
  await unlink(planFile).catch(() => undefined);
  return { ok: true, revision: receipt.after_revision };
}
