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

interface PlanAction {
  action?: unknown; summary?: unknown; name?: unknown;
  before?: Policy; after?: Policy;
}
interface Policy { entities?: unknown; excluded_folders?: unknown; excluded_tags?: unknown; excluded_entities?: unknown }

/** `folder:People` → "the People folder"; `#x` → "notes tagged #x". */
function describeRef(ref: string) {
  if (ref.startsWith("folder:")) return `the ${ref.slice(7)} folder`;
  if (ref.startsWith("#")) return `notes tagged ${ref}`;
  if (ref.startsWith("tag:")) return `notes tagged #${ref.slice(4)}`;
  if (ref.startsWith("source:")) return `the ${ref.slice(7)} source`;
  return ref;
}

/** Readings as ref → options, from the view's `entities` list. */
function readings(value: unknown): Map<string, string> {
  const out = new Map<string, string>();
  for (const item of Array.isArray(value) ? value : []) {
    if (typeof item === "string") out.set(item, "");
    else if (item && typeof item === "object") {
      for (const [ref, options] of Object.entries(item)) out.set(ref, JSON.stringify(options));
    }
  }
  return out;
}
const stringSet = (value: unknown) => new Set(Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : []);

/**
 * Plain-language lines for one plan action. The CLI's summaries are written
 * for operators ("Attention policy: learn questions from folder:People"); the
 * typed before/after views say the same thing in the user's terms. Anything
 * not recognized keeps the CLI's summary, and the diff stays exact.
 */
export function plainSummaries(action: PlanAction): string[] {
  const name = typeof action.name === "string" ? `"${action.name}"` : "a source";
  if (action.action === "add_binding") return [`Add the source ${name}`];
  if (action.action === "remove_binding") return [`Remove the source ${name}`];
  if (action.action === "update_binding") return [`Change the source ${name}`];
  if (action.action === "update_program") return ["Other changes, such as learning settings, profiles, or layout (see the diff)"];
  if (action.action !== "set_policy" || !action.before || !action.after) return [String(action.summary)];
  const lines: string[] = [];
  const before = readings(action.before.entities);
  const after = readings(action.after.entities);
  for (const [ref, options] of after) {
    if (!before.has(ref)) lines.push(`Learn from ${describeRef(ref)} (new)`);
    else if (before.get(ref) !== options) lines.push(`Change how Margins learns from ${describeRef(ref)}`);
  }
  for (const ref of before.keys()) if (!after.has(ref)) lines.push(`Stop learning from ${describeRef(ref)}`);
  const sets: Array<[keyof Policy, (item: string) => string, string, string]> = [
    ["excluded_folders", (item) => `the ${item} folder`, "Leave out", "Stop leaving out"],
    ["excluded_tags", (item) => `notes tagged #${item.replace(/^#/, "")}`, "Leave out", "Stop leaving out"],
    ["excluded_entities", describeRef, "Leave out", "Stop leaving out"],
  ];
  for (const [key, describe, added, removed] of sets) {
    const was = stringSet(action.before[key]);
    const now = stringSet(action.after[key]);
    for (const item of now) if (!was.has(item)) lines.push(`${added} ${describe(item)}`);
    for (const item of was) if (!now.has(item)) lines.push(`${removed} ${describe(item)}`);
  }
  return lines.length ? lines : [String(action.summary)];
}

function programError(error: CliFailure): ProgramError {
  const location = programErrorLocation(error.message);
  return { code: error.code, message: error.message, line: location?.line ?? null, column: location?.column ?? null };
}

export const UPDATE_MARGINS_FOR_PROGRAM = "This Margins version can't edit the Workspace program from bb. Update Margins, then try again.";

export async function readWorkspaceProgram(workspaceId: string): Promise<WorkspaceProgram> {
  requireWorkspaceId(workspaceId);
  const capabilities = JSON.parse(await cli(["capabilities"])) as { workspace?: { program?: unknown } };
  if (capabilities.workspace?.program !== true) throw new Error(UPDATE_MARGINS_FOR_PROGRAM);
  const listing = JSON.parse(await cli(["workspace", "list", "--json"])) as { workspaces?: Array<{ id?: unknown; name?: unknown }> };
  const name = listing.workspaces?.find((item) => item.id === workspaceId)?.name;
  const shown = JSON.parse(await cli(["--workspace", workspaceId, "workspace", "show", "--text", "--json"])) as {
    workspace_id?: unknown; program_path?: unknown; revision?: unknown; program?: unknown;
  };
  if (shown.workspace_id !== workspaceId || typeof shown.program_path !== "string" || !isAbsolute(shown.program_path)
    || typeof shown.revision !== "string" || typeof shown.program !== "string") {
    throw new Error("Margins returned an invalid Workspace program.");
  }
  return { workspaceId, workspaceName: typeof name === "string" && name ? name : null,
    programPath: shown.program_path, revision: shown.revision, program: shown.program };
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
  const actions = Array.isArray(plan.actions) ? plan.actions.flatMap((item) => {
    const action = item as PlanAction;
    return typeof action.action === "string" && typeof action.summary === "string"
      ? plainSummaries(action).map((summary) => ({ action: action.action as string, summary })) : [null];
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
  const text = await readFile(planFile, "utf8").catch(() => null);
  // Plans are pruned after an hour or when many newer ones exist (another
  // open editor plans too); the user's text is untouched, only the review.
  if (text === null) return { ok: false, error: { code: "expired", line: null, column: null,
    message: "This review expired before it was saved. Your text is unchanged; review the changes again." } };
  const plan = JSON.parse(text) as { workspace_id?: unknown };
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
