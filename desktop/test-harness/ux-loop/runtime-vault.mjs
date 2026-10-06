import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const desktopDir = resolve(here, "../..");
const runtimeDir = resolve(here, "runtime");

export async function setupRuntimeVault({ runId, fixture = "customer-call" } = {}) {
  const safeRunId = sanitizeRunId(runId || process.env.MARGINS_UX_E2E_RUN_ID || "latest");
  const fixtureDir = resolve(here, "fixtures", fixture);
  const runDir = resolve(desktopDir, "ux-e2e-runs", safeRunId);
  const vaultDir = resolve(runDir, "runtime-vault");
  const marginsDir = resolve(vaultDir, ".margins");
  const inboxDir = resolve(vaultDir, "inbox");
  const peopleDir = resolve(vaultDir, "people");
  const obsidianDir = resolve(vaultDir, ".obsidian");
  const runtimePath = resolve(runtimeDir, `${safeRunId}.json`);

  const [settingsText, memo, aligned, captureContext] = await Promise.all([
    readFile(resolve(fixtureDir, "settings.json"), "utf8"),
    readFile(resolve(fixtureDir, "memo.md"), "utf8"),
    readFile(resolve(fixtureDir, "aligned.md"), "utf8"),
    readFile(resolve(fixtureDir, "capture-context.md"), "utf8"),
  ]);
  const settings = JSON.parse(settingsText);
  const sessionName = settings.session_name || fixture;
  const startedAt = new Date(Date.now() - 12 * 60_000).toISOString();

  await rm(vaultDir, { recursive: true, force: true });
  await mkdir(runtimeDir, { recursive: true });
  await mkdir(marginsDir, { recursive: true });
  await mkdir(obsidianDir, { recursive: true });
  await mkdir(inboxDir, { recursive: true });
  await mkdir(peopleDir, { recursive: true });

  await Promise.all([
    writeFile(resolve(obsidianDir, "app.json"), "{}\n", "utf8"),
    writeFile(resolve(marginsDir, `${sessionName}_aligned.md`), aligned, "utf8"),
    writeFile(resolve(marginsDir, `${sessionName}_capture-context.md`), captureContext, "utf8"),
    writeFile(resolve(vaultDir, `${sessionName}_memo.md`), memo, "utf8"),
    writeFile(resolve(peopleDir, "Marcus Webb.md"), "# Marcus Webb\n\nDesign partner evaluating Margins for customer calls.\n", "utf8"),
    writeFile(resolve(peopleDir, "Joshua Pham.md"), "# Joshua Pham\n\nMargins product lead.\n", "utf8"),
    writeFile(
      resolve(inboxDir, "pilot-design.md"),
      [
        "---",
        "tags: [margins, pilot, design-partner]",
        "---",
        "",
        "# Pilot design",
        "",
        "Keep the first pilot narrow enough to learn from real customer calls.",
        "The reviewer should trust that Margins used marks, transcript turns, and related notes without reading logs.",
        "",
      ].join("\n"),
      "utf8",
    ),
    writeFile(
      resolve(inboxDir, "operating-cadence.md"),
      [
        "---",
        "tags: [meetings, cadence]",
        "---",
        "",
        "# Operating cadence",
        "",
        "Meeting notes should reduce coordination debt and preserve the decisions people need later.",
        "Related-note language should feel like recognition, not developer plumbing.",
        "",
      ].join("\n"),
      "utf8",
    ),
  ]);

  const seededNotes = [
    {
      path: "inbox/pilot-design.md",
      title: "Pilot design",
      anchors: ["pilot scope", "design partner", "sample note", "sample recording", "trust that Margins used marks"],
    },
    {
      path: "inbox/operating-cadence.md",
      title: "Operating cadence",
      anchors: ["status ritual", "coordination debt", "related-note language", "developer plumbing"],
    },
    {
      path: "people/Marcus Webb.md",
      title: "Marcus Webb",
      anchors: ["design partner", "customer calls"],
    },
    {
      path: "people/Joshua Pham.md",
      title: "Joshua Pham",
      anchors: ["Margins product lead"],
    },
  ];

  const runtime = {
    run_id: safeRunId,
    fixture,
    vault_path: vaultDir,
    cleanup_paths: [vaultDir, runtimePath],
    settings: {
      vault_path: vaultDir,
      projects: [
        {
          id: "runtime-vault",
          name: "UX Runtime Vault",
          path: vaultDir,
          inbox_folder: settings.inbox_folder || "inbox",
          people_folder: settings.people_folder || "people",
          readiness: "ready",
        },
      ],
      active_project_id: "runtime-vault",
      inbox_folder: settings.inbox_folder || "inbox",
      people_folder: settings.people_folder || "people",
      created_date_format: settings.created_date_format || "[[%Y-%m-%d]]",
      note_filename_template: settings.note_filename_template || "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}",
      distill_instructions: settings.distill_instructions || "",
      api_key: "runtime-fixture-key",
      ai_mode: "api",
    },
    sessions: [
      {
        name: sessionName,
        project_id: "runtime-vault",
        start_time: startedAt,
        notes_path: `${sessionName}_memo.md`,
        title: settings.event_title || "Customer call",
        segment_count: 52,
        duration_secs: 713,
        memo_line_count: countMemoLines(memo),
        status: "unprocessed",
        vault_note_path: null,
        people: settings.people || [],
        calendar_event_title: settings.event_title || null,
        source: "session",
        frontmatter_title: null,
        frontmatter_created: null,
        frontmatter_created_sort: null,
        frontmatter_tags: [],
        frontmatter_people: settings.people || [],
        frontmatter_reflection_type: null,
      },
    ],
    samples: {
      [sessionName]: {
        memo,
        aligned,
        capture_context: captureContext,
      },
    },
    vault_notes: seededNotes.map(note => note.path),
    seeded_notes: seededNotes,
  };

  await writeFile(runtimePath, `${JSON.stringify(runtime, null, 2)}\n`, "utf8");
  await writeFile(resolve(runDir, "runtime-vault-manifest.json"), `${JSON.stringify({
    run_id: safeRunId,
    fixture,
    runtime_file: runtimePath,
    vault_path: vaultDir,
    seeded_notes: seededNotes,
    seeded_note_titles: seededNotes.map(note => note.title),
    seeded_note_anchors: [...new Set(seededNotes.flatMap(note => note.anchors))],
  }, null, 2)}\n`, "utf8");

  return runtime;
}

export async function cleanupRuntimeVault({ runId } = {}) {
  const safeRunId = sanitizeRunId(runId || process.env.MARGINS_UX_E2E_RUN_ID || "latest");
  const runtimePath = resolve(runtimeDir, `${safeRunId}.json`);
  const runDir = resolve(desktopDir, "ux-e2e-runs", safeRunId);
  const vaultDir = resolve(runDir, "runtime-vault");

  await Promise.all([
    rm(runtimePath, { force: true }),
    rm(vaultDir, { recursive: true, force: true }),
  ]);
}

function countMemoLines(memo) {
  return memo.split("\n").filter(line => /^\s*(?:[-*]\s+)?\[\d{2}:\d{2}/.test(line)).length || 0;
}

function sanitizeRunId(runId) {
  return String(runId).replace(/[^a-zA-Z0-9._-]/g, "-").slice(0, 120) || "latest";
}
