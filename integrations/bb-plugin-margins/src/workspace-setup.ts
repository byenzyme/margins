import { createHash, randomUUID } from "node:crypto";
import { execFile as execFileCallback } from "node:child_process";
import { lstat, mkdir, readFile, realpath, unlink, writeFile } from "node:fs/promises";
import { basename, isAbsolute, join, relative, resolve } from "node:path";
import { promisify } from "node:util";
import type { ProjectTarget } from "./contracts.js";
import { marginsCli, marginsHome, pendingWorkspaceSetupMarker } from "./project-server.js";

const execFile = promisify(execFileCallback);
const previewIdPattern = /^[0-9a-f]{8}-[0-9a-f-]{27}$/;

export interface WorkspaceSetupPreview {
  previewId: string;
  workspaceId: string;
  homeRoot: string;
  destination: string;
  mode: "jev" | "automatic_fallback" | "empty";
  warning: string | null;
  filesScanned: number;
  selectedEntities: string[];
  actions: unknown[];
}

function setupEnv() { return { ...process.env, MARGINS_HOME: marginsHome() }; }

async function cli(args: string[], timeout = 15_000, maxBuffer = 1_000_000) {
  const { stdout } = await execFile(marginsCli(), args, { env: setupEnv(), timeout, maxBuffer });
  return stdout;
}

async function selectedHome(target: ProjectTarget, input: string): Promise<string> {
  const requested = input.trim();
  if (requested && !isAbsolute(requested)) throw new Error("Choose an absolute notes folder on the Workspace machine.");
  const path = requested || target.projectRoot;
  if (!requested && !(await lstat(join(path, ".obsidian")).catch(() => null))?.isDirectory()) {
    throw new Error("Choose the notes folder on the Workspace machine.");
  }
  const root = await realpath(path);
  if (!(await lstat(root)).isDirectory()) throw new Error("The notes folder is not a directory.");
  const projectRoot = await realpath(target.projectRoot);
  const within = relative(projectRoot, root);
  if (within === ".." || within.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) || isAbsolute(within)) {
    throw new Error("Choose the notes folder as a bb project before setting up its Workspace.");
  }
  return root;
}

function validNoteFolder(value: string) {
  const folder = value.trim();
  if (!folder) return "";
  if (isAbsolute(folder) || folder.split(/[\\/]/).some((part) => !part || part === "." || part === "..")) {
    throw new Error("Note folder must be a folder name relative to Home.");
  }
  return folder;
}

function workspaceIdFor(root: string, existing: Set<string>) {
  const slug = basename(root).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40) || "notes";
  if (!existing.has(slug)) return slug;
  return `${slug}-${createHash("sha256").update(root).digest("hex").slice(0, 8)}`;
}

function entityNames(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => typeof entry === "string" ? [entry]
    : entry && typeof entry === "object" && !Array.isArray(entry) ? Object.keys(entry) : []);
}

export async function previewWorkspaceSetup(
  target: ProjectTarget, dataDir: string, homeInput: string, noteFolderInput: string,
): Promise<WorkspaceSetupPreview> {
  const homeRoot = await selectedHome(target, homeInput);
  const noteFolder = validNoteFolder(noteFolderInput);
  const listing = JSON.parse(await cli(["workspace", "list", "--json"])) as {
    workspaces?: Array<{ id: string }>;
  };
  if (!Array.isArray(listing.workspaces)) throw new Error("Margins Workspace list is unavailable.");
  let workspaceId: string | null = null;
  for (const item of listing.workspaces) {
    if (!/^[a-z0-9][a-z0-9-]*$/.test(item.id)) continue;
    const destination = JSON.parse(await cli(["--workspace", item.id, "workspace", "destination", "--json"])) as { home_root?: string };
    if (destination.home_root && await realpath(destination.home_root).catch(() => null) === homeRoot) {
      workspaceId = item.id;
      break;
    }
  }
  if (!workspaceId) {
    workspaceId = workspaceIdFor(homeRoot, new Set(listing.workspaces.map((item) => item.id)));
    const marker = pendingWorkspaceSetupMarker(workspaceId);
    await mkdir(join(marginsHome(), "pending-workspace-setup"), { recursive: true, mode: 0o700 });
    await writeFile(marker, homeRoot, { mode: 0o600 });
    try {
      await cli(["workspace", "new", workspaceId, "--home", homeRoot, "--name", basename(homeRoot), "--json"]);
    } catch (error) {
      await unlink(marker).catch(() => undefined);
      throw error;
    }
  }
  const compiled = JSON.parse(await cli([
    "--workspace", workspaceId, "workspace", "compile",
    ...(noteFolder ? ["--note-folder", noteFolder] : []), "--json",
  ], 120_000, 2_000_000)) as {
    schema_version?: string; desired_toml?: string; mode?: WorkspaceSetupPreview["mode"];
    warning?: string | null; files_scanned?: number; selected_entities?: unknown;
  };
  if (compiled.schema_version !== "margins.workspace.compile.v1" || typeof compiled.desired_toml !== "string"
    || !["jev", "automatic_fallback", "empty"].includes(compiled.mode || "")) {
    throw new Error("Margins returned an invalid Workspace proposal.");
  }
  const previewId = randomUUID();
  const plansDir = join(dataDir, "setup-plans");
  await mkdir(plansDir, { recursive: true, mode: 0o700 });
  const desiredFile = join(plansDir, `${previewId}.desired.toml`);
  await writeFile(desiredFile, compiled.desired_toml, { mode: 0o600, flag: "wx" });
  const planJson = await cli(["--workspace", workspaceId, "workspace", "plan", "--desired", desiredFile, "--json"]);
  const plan = JSON.parse(planJson) as { schema_version?: string; workspace_id?: string; actions?: unknown[] };
  if (plan.workspace_id !== workspaceId || !Array.isArray(plan.actions)) throw new Error("Margins returned an invalid Workspace plan.");
  await writeFile(join(plansDir, `${previewId}.plan.json`), planJson, { mode: 0o600, flag: "wx" });
  return {
    previewId, workspaceId, homeRoot, destination: noteFolder ? resolve(homeRoot, noteFolder) : homeRoot,
    mode: compiled.mode!, warning: compiled.warning || null,
    filesScanned: Number(compiled.files_scanned || 0), selectedEntities: entityNames(compiled.selected_entities),
    actions: plan.actions,
  };
}

export async function applyWorkspaceSetup(dataDir: string, previewId: string) {
  if (!previewIdPattern.test(previewId)) throw new Error("Invalid Workspace setup preview.");
  const planFile = join(dataDir, "setup-plans", `${previewId}.plan.json`);
  const plan = JSON.parse(await readFile(planFile, "utf8")) as { workspace_id?: string };
  if (!plan.workspace_id || !/^[a-z0-9][a-z0-9-]*$/.test(plan.workspace_id)) {
    throw new Error("Invalid Workspace setup plan.");
  }
  await cli(["--workspace", plan.workspace_id, "workspace", "apply", "--plan", planFile, "--json"]);
  await cli(["workspace", "default", "--set", plan.workspace_id, "--json"]);
  await unlink(pendingWorkspaceSetupMarker(plan.workspace_id)).catch((error: NodeJS.ErrnoException) => {
    if (error.code !== "ENOENT") throw error;
  });
  const destination = JSON.parse(await cli(["--workspace", plan.workspace_id, "workspace", "destination", "--json"])) as { destination?: string };
  if (!destination.destination || !isAbsolute(destination.destination)) throw new Error("Workspace destination is unavailable after setup.");
  return { workspaceId: plan.workspace_id, destination: destination.destination };
}
