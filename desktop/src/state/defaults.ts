import type { ProjectSource, Settings } from "../lib/tauri";

export const DEFAULT_VAULT_PATH = "~/Documents/margins";
export const SETUP_RESUME_SECTION_KEY = "margins.setup.resume";

export const EDITOR_OPTIONS = [
  { value: "system", label: "Default system app" },
  { value: "obsidian", label: "Obsidian" },
  { value: "vscode", label: "Visual Studio Code" },
  { value: "textedit", label: "TextEdit" },
];

export const DEFAULT_DISTILL_INSTRUCTIONS = "Create the final meeting note with created date and attendee people links.";

export function projectIdFromPath(path: string): string {
  const slug = path
    .replace(/^~\//, "")
    .replace(/[^a-zA-Z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .toLowerCase()
    .slice(0, 42);
  return slug || "project";
}

export function projectNameFromPath(path: string): string {
  const clean = path.replace(/\/+$/g, "");
  const leaf = clean.split(/[\\/]+/).filter(Boolean).pop() || "Project";
  return leaf.replace(/[-_]+/g, " ").replace(/\b\w/g, c => c.toUpperCase());
}

export function defaultProject(path = DEFAULT_VAULT_PATH, inboxFolder = "meetings"): ProjectSource {
  return {
    id: projectIdFromPath(path),
    name: projectNameFromPath(path),
    path,
    inbox_folder: inboxFolder,
    people_folder: "people",
    readiness: "needs_setup",
  };
}

export function normalizeProjects(raw: Settings): ProjectSource[] {
  const projects = Array.isArray(raw.projects) ? raw.projects : [];
  const normalized = projects
    .map((project, index) => {
      const path = project.path?.trim() || (index === 0 ? raw.vault_path || DEFAULT_VAULT_PATH : "");
      if (!path) return null;
      return {
        id: project.id?.trim() || projectIdFromPath(path),
        name: project.name?.trim() || projectNameFromPath(path),
        path,
        inbox_folder: project.inbox_folder ?? raw.inbox_folder ?? "meetings",
        people_folder: project.people_folder ?? raw.people_folder ?? "people",
        readiness: project.readiness || "needs_setup",
      } satisfies ProjectSource;
    })
    .filter(Boolean) as ProjectSource[];
  if (normalized.length > 0) return normalized;
  return [defaultProject(raw.vault_path || DEFAULT_VAULT_PATH, raw.inbox_folder ?? "meetings")];
}

export function activeProject(settings: Settings): ProjectSource {
  const projects = normalizeProjects(settings);
  return projects.find(p => p.id === settings.active_project_id) || projects[0];
}

export function defaultSettings(): Settings {
  return {
    vault_path: DEFAULT_VAULT_PATH,
    projects: [defaultProject(DEFAULT_VAULT_PATH, "meetings")],
    active_project_id: projectIdFromPath(DEFAULT_VAULT_PATH),
    ai_mode: "included",
    ai_base_url: null,
    ai_model: null,
    api_key: null,
    backchannel_base_url: null,
    backchannel_model: null,
    backchannel_api_key: null,
    backchannel_same_as_distill: true,
    cleanup_policy: "immediate",
    input_device_mode: "follow_default",
    input_device_uid: null,
    input_device_name: null,
    audio_input_ready: false,
    system_audio_ready: false,
    editor_command: null,
    parakeet_model_dir: null,
    rust_diarization_enabled: false,
    inbox_folder: "meetings",
    people_folder: "people",
    created_date_format: "[[%Y-%m-%d]]",
    sidebar_date_format: "compact",
    note_filename_template: "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}",
    person_note_template: "# {{name}}\n",
    distill_instructions: DEFAULT_DISTILL_INSTRUCTIONS,
  };
}

// Stage a project folder change in memory without persisting to disk.
// All settings fields outside the project path are preserved exactly.
// Callers must call updateSettings to commit the change.
export function stageProjectPathChange(current: Settings, projectId: string, newPath: string): Settings {
  const projects = normalizeProjects(current).map(p =>
    p.id === projectId
      ? { ...p, path: newPath, name: p.name || projectNameFromPath(newPath) }
      : p
  );
  return normalizeSettings({ ...current, projects });
}

// Resolve the `editor_command` to persist from the settings pane. The editor
// <select> cannot represent "unset": a null/absent stored value is rendered as
// the "system" option (see `selectedEditorValue` in main.ts). So a Save that
// never touched the editor picker must NOT bake in a "system" default the user
// never chose — that is the Notes-Change-Save regression where editor_command
// flips null -> "system". Preserve the stored value (including null) whenever the
// DOM value still equals the render-time baseline (untouched) or the select was
// not rendered at all. Only a value that differs from the baseline is treated as
// an explicit user choice. Merge-not-replace, all the way down.
export function resolveEditorCommand(
  domValue: string | null | undefined,
  renderedBaseline: string,
  stored: string | null | undefined,
): string | null {
  if (domValue == null) return stored ?? null; // select not in the rendered pane
  if (domValue === renderedBaseline) return stored ?? null; // untouched by the user
  return domValue; // explicit pick
}

export function normalizeSettings(raw: Settings): Settings {
  const projects = normalizeProjects(raw);
  const active = projects.find(p => p.id === raw.active_project_id) || projects[0];
  return {
    ...raw,
    vault_path: active?.path || raw.vault_path || DEFAULT_VAULT_PATH,
    projects,
    active_project_id: active?.id || null,
    ai_mode: raw.ai_mode ?? (raw.api_key ? "api" : "included"),
    ai_base_url: raw.ai_base_url ?? null,
    ai_model: raw.ai_model ?? null,
    backchannel_base_url: raw.backchannel_base_url ?? null,
    backchannel_model: raw.backchannel_model ?? null,
    backchannel_api_key: raw.backchannel_api_key ?? null,
    backchannel_same_as_distill: raw.backchannel_same_as_distill ?? true,
    input_device_mode: raw.input_device_mode ?? (raw.input_device_uid ? "pinned" : "follow_default"),
    input_device_uid: raw.input_device_uid ?? null,
    audio_input_ready: raw.audio_input_ready ?? false,
    system_audio_ready: raw.system_audio_ready ?? false,
    editor_command: raw.editor_command ?? null,
    parakeet_model_dir: raw.parakeet_model_dir ?? null,
    rust_diarization_enabled: raw.rust_diarization_enabled ?? false,
    inbox_folder: active?.inbox_folder ?? raw.inbox_folder ?? "meetings",
    people_folder: active?.people_folder || raw.people_folder || "people",
    created_date_format: raw.created_date_format || "[[%Y-%m-%d]]",
    sidebar_date_format: raw.sidebar_date_format || "compact",
    note_filename_template: raw.note_filename_template || "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}",
    person_note_template: raw.person_note_template || "# {{name}}\n",
    distill_instructions: raw.distill_instructions || DEFAULT_DISTILL_INSTRUCTIONS,
  };
}
