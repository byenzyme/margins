import assert from "node:assert/strict";
import test from "node:test";

import { defaultSettings, stageProjectPathChange, normalizeProjects, activeProject, resolveEditorCommand } from "../src/state/defaults.ts";
import type { Settings } from "../src/lib/tauri.ts";

function vaultSettings(overrides: Partial<Settings> = {}): Settings {
  return {
    ...defaultSettings(),
    projects: [{ id: "my-vault", name: "My Vault", path: "/old/path", inbox_folder: "meetings", people_folder: "people", readiness: "ready" }],
    active_project_id: "my-vault",
    audio_input_ready: true,
    system_audio_ready: true,
    parakeet_model_dir: "/models/parakeet-tdt-0.6b-v2",
    input_device_mode: "pinned",
    input_device_uid: "BLUEYETI-UID",
    input_device_name: "Blue Yeti",
    editor_command: null,
    ...overrides,
  };
}

test("stageProjectPathChange updates only the project path, preserves audio and model settings", () => {
  const before = vaultSettings();
  const after = stageProjectPathChange(before, "my-vault", "/new/notes-vault");

  assert.equal(activeProject(after).path, "/new/notes-vault", "project path updated");
  assert.equal(after.vault_path, "/new/notes-vault", "vault_path derived from project path");
  assert.equal(after.audio_input_ready, true, "audio_input_ready preserved");
  assert.equal(after.system_audio_ready, true, "system_audio_ready preserved");
  assert.equal(after.parakeet_model_dir, "/models/parakeet-tdt-0.6b-v2", "parakeet_model_dir preserved");
  assert.equal(after.input_device_mode, "pinned", "input_device_mode preserved");
  assert.equal(after.input_device_uid, "BLUEYETI-UID", "input_device_uid preserved");
  assert.equal(after.input_device_name, "Blue Yeti", "input_device_name preserved");
  assert.equal(after.active_project_id, "my-vault", "active_project_id unchanged");
  assert.equal(after.editor_command, null, "editor_command null preserved (not defaulted to system)");
});

test("resolveEditorCommand preserves unset/null unless the user explicitly picks", () => {
  // The Notes-Change-Save regression: the editor <select> renders a null
  // editor_command as the "system" option, so an untouched Save must not commit
  // "system". domValue === renderedBaseline means the user never touched it.
  assert.equal(resolveEditorCommand("system", "system", null), null, "untouched null stays null");
  assert.equal(resolveEditorCommand("system", "system", undefined), null, "untouched undefined stays null");

  // Select not rendered in the active pane: preserve the stored value verbatim.
  assert.equal(resolveEditorCommand(undefined, "system", null), null, "unrendered null preserved");
  assert.equal(resolveEditorCommand(undefined, "vscode", "code /Applications/Cursor"), "code /Applications/Cursor", "unrendered custom command preserved");

  // Untouched pane whose stored value is a custom command mapped to a baseline
  // option: preserve the exact stored command, not the coarse baseline.
  assert.equal(resolveEditorCommand("vscode", "vscode", "code /Applications/Cursor"), "code /Applications/Cursor", "untouched custom command preserved");

  // Explicit user pick (differs from baseline) is honored.
  assert.equal(resolveEditorCommand("obsidian", "system", null), "obsidian", "explicit pick honored over null");
  assert.equal(resolveEditorCommand("textedit", "system", "obsidian"), "textedit", "explicit pick honored over stored");
});

test("stageProjectPathChange leaves other projects and unrelated settings unchanged", () => {
  const before: Settings = {
    ...defaultSettings(),
    projects: [
      { id: "vault-a", name: "Vault A", path: "/vault-a", inbox_folder: "meetings", people_folder: "people", readiness: "ready" },
      { id: "vault-b", name: "Vault B", path: "/vault-b", inbox_folder: "notes", people_folder: "people", readiness: "needs_setup" },
    ],
    active_project_id: "vault-a",
    parakeet_model_dir: "/models/parakeet",
    audio_input_ready: true,
    system_audio_ready: true,
    distill_instructions: "custom distill instructions",
  };

  const after = stageProjectPathChange(before, "vault-a", "/vault-a-new");

  const projects = normalizeProjects(after);
  assert.equal(projects.find(p => p.id === "vault-a")?.path, "/vault-a-new", "target project path updated");
  assert.equal(projects.find(p => p.id === "vault-b")?.path, "/vault-b", "other project path unchanged");
  assert.equal(after.parakeet_model_dir, "/models/parakeet", "model dir preserved");
  assert.equal(after.audio_input_ready, true, "audio readiness preserved");
  assert.equal(after.system_audio_ready, true, "system audio readiness preserved");
  assert.equal(after.distill_instructions, "custom distill instructions", "distill instructions preserved");
});

test("stageProjectPathChange on unknown projectId leaves all paths unchanged", () => {
  const before = vaultSettings();
  const after = stageProjectPathChange(before, "nonexistent-id", "/new/path");

  assert.equal(activeProject(after).path, "/old/path", "active project path unchanged");
  assert.equal(after.audio_input_ready, true, "audio readiness unchanged");
  assert.equal(after.parakeet_model_dir, "/models/parakeet-tdt-0.6b-v2", "model dir unchanged");
});
