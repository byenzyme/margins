//! `margins status`: one read-only view of the selected Workspace, ordered
//! by what a person needs first: which Workspace, whether its index and
//! catalysts are ready (shown separately, so an index without catalysts is
//! never reported as full recall), what Margins learns about, then Sources,
//! connections, and captures.
//!
//! The public composition fills what it can read without the engine; the
//! official composition adds the engine's index, selection, and `--explain`
//! detail through [`EngineView`]. `--json` is `margins.status.v1`; the older
//! `workspace status --json` shape is unchanged and separate.

use super::workspace_text::{self, reading_label};
use crate::error::CliError;
use margins_workflows::workspace::{
    self, ResolvedWorkspace, SourceRole, WorkspaceBinding, WorkspaceConfig,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

pub const STATUS_SCHEMA: &str = "margins.status.v1";

/// Automatic picks listed before "+N more" without `--all`.
pub const AUTOMATIC_SHOWN: usize = 10;

/// How the Workspace was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectedBy {
    /// `--workspace` or `MARGINS_WORKSPACE`.
    Selection,
    /// It declares the current folder.
    Folder,
    /// The machine default.
    Default,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub schema_version: &'static str,
    pub workspace: WorkspaceView,
    pub index: IndexView,
    pub catalysts: CatalystView,
    pub attention: AttentionView,
    pub leaves_out: Vec<String>,
    pub notes_go_to: Option<String>,
    pub sources: Vec<SourceRow>,
    pub connections: Vec<ConnectionRow>,
    pub integrations: Vec<Value>,
    pub captures: CapturesView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain: Option<ExplainView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceView {
    pub id: String,
    pub name: Option<String>,
    pub home: String,
    pub program: String,
    pub revision: String,
    pub is_default: bool,
    pub selected_by: SelectedBy,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IndexView {
    /// `indexed`, `not_built`, `outdated`, or `lexical` (public build: no
    /// engine index; notes are searched as they are).
    pub state: String,
    pub documents: usize,
    /// Notes changed since the index was last refreshed; `None` when unknown.
    pub notes_changed: Option<NotesChanged>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct NotesChanged {
    pub new: usize,
    pub modified: usize,
    pub deleted: usize,
}

impl NotesChanged {
    pub fn total(&self) -> usize {
        self.new + self.modified + self.deleted
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CatalystView {
    /// The generator setup selected: `hosted`, `local`, or `none`.
    pub mode: String,
    /// Whether that generator can run now.
    pub usable: bool,
    pub reason: String,
    /// Catalysts in the index.
    pub catalysts: usize,
    /// Selected entities by state.
    pub ready: usize,
    pub pending: usize,
    pub skipped: usize,
    /// The one command that moves catalysts forward, when one does.
    pub next_step: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AttentionView {
    /// What the program asks Margins to learn from, in program order.
    pub readings: Vec<ReadingView>,
    /// Whether the program also lets Margins pick automatically.
    pub learns_automatically: bool,
    /// Entities the engine selected from the readings.
    pub from_readings: Vec<EntityView>,
    /// Entities the engine picked automatically (capped unless `--all`).
    pub automatic: Vec<EntityView>,
    pub automatic_total: usize,
    /// Skipped entities by reason kind.
    pub skipped_by_reason: BTreeMap<String, usize>,
    /// Whether lists were cut short (pass `--all`).
    pub truncated: bool,
    /// `false` when the engine selection was not read (public build, or no
    /// index yet).
    pub known: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadingView {
    /// As written in the program, e.g. `folder:Meetings`.
    pub reading: String,
    pub label: String,
    pub about: Option<String>,
    pub including_linked_pages: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct EntityView {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub name: String,
    /// `reading` or `automatic`.
    pub origin: String,
    /// `ready`, `pending`, `unchecked`, or `skipped`.
    pub state: String,
    pub catalysts: usize,
    pub skip_kind: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceRow {
    pub name: String,
    pub kind: String,
    pub location: String,
    pub documents: Option<usize>,
    pub stale: bool,
    pub stale_reason: Option<String>,
    pub last_refresh_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionRow {
    pub service: String,
    pub account: String,
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CapturesView {
    pub sessions: usize,
    pub current: Option<String>,
    pub latest: Option<String>,
}

/// `--explain`: per reading, what the next catalyst build would learn about
/// and why some of it yields nothing, from the engine's read-only `spec plan`
/// (no model calls). Reasons are in Margins' words ([`why`]).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ExplainView {
    /// One group per reading in program order, then `picked automatically`.
    pub readings: Vec<ExplainReading>,
    /// Entities that get a catalyst job.
    pub planned: usize,
    /// Selected entities that get none.
    pub skipped: usize,
    /// Linked pages or matches not selected at all.
    pub candidates_skipped: usize,
    /// The engine listed only part of a long plan.
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ExplainReading {
    /// The reading as the program words it, or `picked automatically`.
    pub reading: String,
    /// What it learns about (gets a catalyst job).
    pub learns: Vec<String>,
    pub skipped: Vec<ExplainSkip>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExplainSkip {
    /// The engine's reason code (`thin_context`, `left_out`, …).
    pub code: String,
    /// What is skipped: an entity name, or e.g. "4 linked pages".
    pub what: String,
    /// Why, for people.
    pub why: String,
}

/// An engine skip reason in Margins' words. Unknown codes fall back to the
/// engine's own sentence unless it talks about tokens.
pub fn why(code: &str, engine_reason: Option<&str>) -> String {
    let known = match code {
        "not_indexed" => "no note has it yet",
        "left_out" => "the program leaves it out",
        "no_linked_pages" => "none of the folder's pages are linked from your notes yet",
        "no_matches" => "nothing matched",
        "below_link_frequency" => "linked fewer than 3 times",
        "inactive" => "no activity in the last year",
        "below_density_gate" => "too few linked pages for the folder's size, so the folder is read as a whole",
        "own_reading" => "covered by their own reading",
        "cap_reached" => "more than the reading's limit, or readings fill every automatic slot",
        "no_dated_evidence" => "no dated notes yet",
        "no_occurrences" => "not mentioned in any note",
        "no_usable_evidence" => "its mentions gave nothing usable to learn from",
        "thin_context" => "too little written about it yet",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    match engine_reason {
        Some(reason) if !reason.to_ascii_lowercase().contains("token") => reason.to_string(),
        _ => format!("skipped ({})", code.replace('_', " ")),
    }
}

/// What the official composition reads from the engine for one Workspace.
#[derive(Debug, Clone, Default)]
pub struct EngineView {
    pub index: IndexView,
    pub entities: Vec<EntityView>,
    pub catalysts: usize,
    pub sources: BTreeMap<String, (usize, bool, Option<String>, Option<i64>)>,
    pub explain: Option<ExplainView>,
}

/// The Workspace `status` shows, and how it was chosen: the selected one,
/// else the one declaring `cwd`, else the machine default. Read-only.
pub fn select(selector: Option<&str>, cwd: &Path) -> Result<(ResolvedWorkspace, SelectedBy), CliError> {
    let explicit = selector.is_some_and(|value| !value.trim().is_empty())
        || std::env::var("MARGINS_WORKSPACE").is_ok_and(|value| !value.trim().is_empty());
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let selected = workspace::inspect_workspace_or_default(&margins_home, selector, cwd)
        .map_err(CliError::from_anyhow)?
        .ok_or_else(|| {
            CliError::new("workspace_required", super::workspace::NO_WORKSPACE_MESSAGE)
        })?;
    let by = if explicit {
        SelectedBy::Selection
    } else if selected.via_default {
        SelectedBy::Default
    } else {
        SelectedBy::Folder
    };
    Ok((selected.workspace, by))
}

/// Build the report. `engine` is `None` in the public composition.
#[allow(clippy::too_many_arguments)]
pub fn build(
    workspace: &ResolvedWorkspace,
    selected_by: SelectedBy,
    catalyst: CatalystView,
    engine: Option<EngineView>,
    all: bool,
    connections: Vec<ConnectionRow>,
    integrations: Vec<Value>,
    captures: CapturesView,
) -> Result<StatusReport, CliError> {
    let id = &workspace.config.id;
    let revision = workspace::workspace_revision(workspace).map_err(CliError::from_anyhow)?;
    let is_default = workspace_text::is_machine_default(id);
    let config = &workspace.config;
    let mut attention = AttentionView {
        readings: readings(config),
        learns_automatically: config.policy.automatic.is_some(),
        ..AttentionView::default()
    };
    let mut catalyst = catalyst;
    let (index, engine_sources, explain) = match engine {
        Some(engine) => {
            attention.known = engine.index.state == "indexed";
            catalyst.catalysts = engine.catalysts;
            for entity in &engine.entities {
                match entity.state.as_str() {
                    "ready" => catalyst.ready += 1,
                    "skipped" => {
                        catalyst.skipped += 1;
                        *attention
                            .skipped_by_reason
                            .entry(entity.skip_kind.clone().unwrap_or_else(|| "skipped".into()))
                            .or_insert(0) += 1;
                    }
                    _ => catalyst.pending += 1,
                }
            }
            let (from_readings, automatic): (Vec<_>, Vec<_>) = engine
                .entities
                .into_iter()
                .partition(|entity| entity.origin != "automatic");
            attention.from_readings = from_readings;
            attention.automatic_total = automatic.len();
            attention.automatic = automatic;
            if !all && attention.automatic.len() > AUTOMATIC_SHOWN {
                attention.automatic.truncate(AUTOMATIC_SHOWN);
                attention.truncated = true;
            }
            (engine.index, engine.sources, engine.explain)
        }
        None => (
            IndexView {
                state: "lexical".to_string(),
                documents: margins_workflows::local_recall::status(workspace)
                    .map(|status| status.documents)
                    .unwrap_or(0),
                notes_changed: None,
            },
            BTreeMap::new(),
            None,
        ),
    };
    // The engine reports SQLite-backed Sources; with one Markdown Source,
    // the rest of the index is its notes.
    let markdown_sources = config
        .bindings
        .values()
        .filter(|binding| matches!(binding, WorkspaceBinding::NativeMarkdown { .. }))
        .count();
    let markdown_documents = (index.state == "indexed" && markdown_sources == 1).then(|| {
        index
            .documents
            .saturating_sub(engine_sources.values().map(|(documents, ..)| documents).sum())
    });
    let sources = config
        .bindings
        .iter()
        .map(|(name, binding)| {
            let engine = engine_sources.get(name);
            SourceRow {
                name: name.clone(),
                kind: source_kind_label(binding).to_string(),
                location: source_location(binding),
                documents: engine.map(|(documents, ..)| *documents).or(
                    match binding {
                        WorkspaceBinding::NativeMarkdown { .. } => markdown_documents,
                        _ => None,
                    },
                ),
                stale: engine.is_some_and(|(_, stale, ..)| *stale),
                stale_reason: engine.and_then(|(_, _, reason, _)| reason.clone()),
                last_refresh_ms: engine.and_then(|(.., last)| *last),
            }
        })
        .collect();
    Ok(StatusReport {
        schema_version: STATUS_SCHEMA,
        workspace: WorkspaceView {
            id: id.clone(),
            name: config.name.clone(),
            home: workspace.home_dir.display().to_string(),
            program: workspace.config_path.display().to_string(),
            revision,
            is_default,
            selected_by,
        },
        index,
        catalysts: catalyst,
        attention,
        leaves_out: leaves_out(config),
        notes_go_to: workspace
            .note_destination()
            .ok()
            .map(|path| path.display().to_string()),
        sources,
        connections,
        integrations,
        captures,
        explain,
    })
}

fn readings(config: &WorkspaceConfig) -> Vec<ReadingView> {
    config
        .policy
        .entities
        .iter()
        .flat_map(|entity| {
            entity
                .entries()
                .into_iter()
                .map(|(entity_ref, options)| ReadingView {
                    reading: entity_ref.to_string(),
                    label: reading_label(entity_ref).to_string(),
                    about: options.and_then(|options| options.profile.clone()),
                    including_linked_pages: options
                        .is_some_and(|options| options.expandable),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn leaves_out(config: &WorkspaceConfig) -> Vec<String> {
    config
        .policy
        .excluded_folders
        .iter()
        .cloned()
        .chain(
            config
                .policy
                .excluded_tags
                .iter()
                .map(|tag| format!("#{}", tag.trim_start_matches('#'))),
        )
        .chain(
            config
                .policy
                .excluded_entities
                .iter()
                .map(|entity_ref| reading_label(entity_ref).to_string()),
        )
        .collect()
}

fn source_kind_label(binding: &WorkspaceBinding) -> &'static str {
    match binding {
        WorkspaceBinding::NativeMarkdown {
            role: SourceRole::Home,
            ..
        } => "notes (home)",
        WorkspaceBinding::NativeMarkdown { .. } => "notes (reference)",
        WorkspaceBinding::Captures { .. } => "captures",
        WorkspaceBinding::Gmail { .. } => "google mail",
        WorkspaceBinding::GoogleCalendar { .. } => "google calendar",
        WorkspaceBinding::GoogleMeet { .. } => "google meet",
        WorkspaceBinding::Granola { .. } => "granola",
    }
}

fn source_location(binding: &WorkspaceBinding) -> String {
    match binding.local_path() {
        Some(path) => path.display().to_string(),
        None => binding
            .google_account()
            .map(str::to_string)
            .or_else(|| match binding {
                WorkspaceBinding::Granola { account, .. } => Some(account.clone()),
                _ => None,
            })
            .unwrap_or_default(),
    }
}

/// Machine connections from the `connect status --json` envelopes.
pub fn connections(margins_home: &Path) -> Vec<ConnectionRow> {
    let mut rows = Vec::new();
    for (service, run) in [
        ("google", super::connect::status as fn(&Path, bool, &mut dyn Write) -> Result<(), CliError>),
        ("granola", super::connect::granola_status),
    ] {
        let mut buffer = Vec::new();
        if run(margins_home, true, &mut buffer).is_err() {
            rows.push(ConnectionRow {
                service: service.to_string(),
                account: String::new(),
                status: "unreadable".to_string(),
            });
            continue;
        }
        let value: Value = serde_json::from_slice(&buffer).unwrap_or(Value::Null);
        for connection in value["connections"].as_array().into_iter().flatten() {
            rows.push(ConnectionRow {
                service: service.to_string(),
                account: connection["account"].as_str().unwrap_or_default().to_string(),
                status: connection["status"].as_str().unwrap_or("unknown").to_string(),
            });
        }
    }
    rows
}

/// Declared connector health from `integrations status --json`.
pub fn integrations(workspace: &ResolvedWorkspace) -> Vec<Value> {
    let mut buffer = Vec::new();
    if super::integrations::status(&workspace.state_dir, true, &mut buffer).is_err() {
        return Vec::new();
    }
    serde_json::from_slice::<Value>(&buffer)
        .ok()
        .and_then(|value| value.get("results").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

/// Sessions in the Workspace's captures store.
pub fn captures(services: &crate::CliServices, workspace: &ResolvedWorkspace) -> CapturesView {
    let Ok(root) = workspace.capture_store_dir() else {
        return CapturesView::default();
    };
    let margins_dir = root.join(".margins");
    if !margins_dir.is_dir() {
        return CapturesView::default();
    }
    let sessions = services.sessions.list(&margins_dir).unwrap_or_default();
    CapturesView {
        sessions: sessions.len(),
        current: services.sessions.current(&margins_dir).ok().flatten(),
        latest: sessions
            .iter()
            .max_by(|a, b| a.start_time.cmp(&b.start_time))
            .map(|session| session.name.clone()),
    }
}

/// `margins status`: readable, or `margins.status.v1` with `json`.
pub fn write(report: &StatusReport, json: bool, all: bool, out: &mut dyn Write) -> Result<(), CliError> {
    let output = |error: std::io::Error| CliError::new("output_failed", error.to_string());
    if json {
        serde_json::to_writer_pretty(&mut *out, report)
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        return writeln!(out).map_err(output);
    }
    write_text(report, all, out).map_err(output)
}

fn write_text(report: &StatusReport, all: bool, out: &mut dyn Write) -> std::io::Result<()> {
    let ws = &report.workspace;
    let margins = workspace_text::margins_for(&ws.id, ws.is_default);
    let how = match ws.selected_by {
        SelectedBy::Selection => "selected",
        SelectedBy::Folder => "covers this folder",
        SelectedBy::Default => "your default",
    };
    writeln!(out, "Workspace {} ({how}) — notes in {}", ws.id, ws.home)?;
    writeln!(
        out,
        "  Program: {} · revision {}",
        ws.program,
        ws.revision.get(..12).unwrap_or(&ws.revision)
    )?;
    writeln!(out)?;

    // Index and catalysts are separate lines: an index without catalysts
    // finds direct matches only, and must not read as full recall.
    let index = &report.index;
    match index.state.as_str() {
        "indexed" => {
            let changed = index
                .notes_changed
                .as_ref()
                .filter(|changed| changed.total() > 0)
                .map(|changed| {
                    format!(
                        " · {} {} changed since the last sync — run `{margins} sync`",
                        changed.total(),
                        if changed.total() == 1 { "note" } else { "notes" }
                    )
                })
                .unwrap_or_else(|| " · up to date".to_string());
            writeln!(
                out,
                "Index: {} {} indexed{changed}",
                index.documents,
                if index.documents == 1 { "note" } else { "notes" }
            )?;
        }
        "outdated" => writeln!(
            out,
            "Index: built by an older engine — run `{margins} init` to rebuild it"
        )?,
        "lexical" => writeln!(
            out,
            "Index: none in this build; recall reads {} {} directly",
            index.documents,
            if index.documents == 1 { "note" } else { "notes" }
        )?,
        _ => writeln!(out, "Index: not built yet — run `{margins} init`")?,
    }
    let catalysts = &report.catalysts;
    let state = if !catalysts.usable {
        match catalysts.mode.as_str() {
            "none" => "not set up — search finds direct matches only".to_string(),
            mode => format!("{mode}, not usable ({}) — search finds direct matches only", catalysts.reason),
        }
    } else if report.index.state != "indexed" {
        format!("{}, ready to build at the next index", catalysts.mode)
    } else {
        let mut parts = vec![format!(
            "{}, {} {}",
            catalysts.mode,
            catalysts.catalysts,
            if catalysts.catalysts == 1 { "catalyst" } else { "catalysts" }
        )];
        if catalysts.pending > 0 {
            parts.push(format!("{} still building", catalysts.pending));
        }
        parts.join(" · ")
    };
    writeln!(out, "Catalysts: {state}")?;
    if let Some(step) = &catalysts.next_step {
        writeln!(out, "  Next: {step}")?;
    }
    writeln!(out)?;

    let attention = &report.attention;
    writeln!(out, "Learns about:")?;
    if attention.readings.is_empty() {
        if attention.learns_automatically {
            writeln!(out, "  Whatever it picks automatically")?;
        } else {
            writeln!(out, "  Nothing declared yet — add readings with `{margins} edit`")?;
        }
    }
    for reading in &attention.readings {
        let mut line = format!("  {}", reading.label);
        if reading.including_linked_pages {
            line.push_str(", including linked pages");
        }
        if let Some(about) = &reading.about {
            line.push_str(&format!(" — {about}"));
        }
        writeln!(out, "{line}")?;
    }
    if attention.learns_automatically && !attention.readings.is_empty() {
        writeln!(out, "  …plus what those miss, picked automatically")?;
    }
    if attention.known {
        let waiting = if report.catalysts.usable { "building" } else { "waiting for catalysts" };
        if !attention.from_readings.is_empty() {
            writeln!(
                out,
                "  From your readings: {}",
                entity_groups(&attention.from_readings, all, waiting)
            )?;
        }
        if attention.automatic_total > 0 {
            let more = attention.automatic_total - attention.automatic.len();
            writeln!(
                out,
                "  Picked automatically ({}): {}{}",
                attention.automatic_total,
                entity_groups(&attention.automatic, true, waiting),
                if more > 0 {
                    format!(" · +{more} more (--all)")
                } else {
                    String::new()
                }
            )?;
        }
        let skipped: usize = attention.skipped_by_reason.values().sum();
        if skipped > 0 {
            let reasons = attention
                .skipped_by_reason
                .iter()
                .map(|(kind, count)| format!("{count} {}", why(kind, None)))
                .collect::<Vec<_>>()
                .join("; ");
            writeln!(out, "  Skipped: {skipped} ({reasons}) — see why with `{margins} status --explain`")?;
        }
    }
    writeln!(
        out,
        "Leaves out: {}",
        if report.leaves_out.is_empty() {
            "nothing".to_string()
        } else {
            report.leaves_out.join(" · ")
        }
    )?;
    if let Some(destination) = &report.notes_go_to {
        writeln!(out, "New notes go to: {destination}")?;
    }

    if let Some(explain) = &report.explain {
        writeln!(out)?;
        let mut totals = vec![format!(
            "{} {} learned about",
            explain.planned,
            if explain.planned == 1 { "thing" } else { "things" }
        )];
        if explain.skipped > 0 {
            totals.push(format!("{} skipped", explain.skipped));
        }
        if explain.candidates_skipped > 0 {
            totals.push(format!("{} not picked", explain.candidates_skipped));
        }
        writeln!(out, "Why (the next catalyst build: {}):", totals.join(", "))?;
        for reading in &explain.readings {
            let learns = if reading.learns.is_empty() {
                "nothing yet".to_string()
            } else {
                reading.learns.join(", ")
            };
            writeln!(out, "  {}: {learns}", reading.reading)?;
            for skip in &reading.skipped {
                writeln!(out, "    {} — {}", skip.what, skip.why)?;
            }
        }
        if explain.truncated {
            writeln!(out, "  (the engine listed part of a long plan)")?;
        }
    }

    writeln!(out)?;
    writeln!(out, "Sources:")?;
    for source in &report.sources {
        let mut line = format!("  {} — {}", source.name, source.kind);
        if !source.location.is_empty() {
            line.push_str(&format!(", {}", source.location));
        }
        if let Some(documents) = source.documents {
            line.push_str(&format!(" · {documents} indexed"));
        }
        if source.stale {
            line.push_str(&format!(
                " · stale ({})",
                source.stale_reason.as_deref().unwrap_or("refresh needed")
            ));
        }
        writeln!(out, "{line}")?;
    }
    for row in &report.integrations {
        if let (Some(binding), Some(status)) = (
            row["binding"].as_str(),
            row["health"]["status"].as_str(),
        ) {
            writeln!(out, "  {binding}: {status}")?;
        }
    }
    let connections = if report.connections.is_empty() {
        "none — `margins connect google` or `margins connect granola`".to_string()
    } else {
        report
            .connections
            .iter()
            .map(|row| format!("{} {} ({})", row.service, row.account, row.status.replace('_', " ")))
            .collect::<Vec<_>>()
            .join(" · ")
    };
    writeln!(out, "Connections: {connections}")?;
    let captures = &report.captures;
    match (captures.sessions, &captures.latest) {
        (0, _) => writeln!(out, "Captures: none yet — `margins new` starts one")?,
        (count, latest) => writeln!(
            out,
            "Captures: {count} {}{}{}",
            if count == 1 { "session" } else { "sessions" },
            latest
                .as_deref()
                .map(|latest| format!(" · latest {latest}"))
                .unwrap_or_default(),
            captures
                .current
                .as_deref()
                .map(|current| format!(" · current {current}"))
                .unwrap_or_default()
        )?,
    }
    writeln!(out)?;
    if report.explain.is_none() {
        writeln!(out, "See why: `{margins} status --explain` · Change it: `{margins} edit`")
    } else {
        writeln!(out, "Change it: `{margins} edit`")
    }
}

/// Entities grouped by state: "ready: a, b · building: c · skipped: d (thin
/// context)".
fn entity_groups(entities: &[EntityView], all: bool, waiting: &str) -> String {
    let limit = if all { usize::MAX } else { AUTOMATIC_SHOWN };
    let names = |state: &dyn Fn(&EntityView) -> bool, skip: bool| {
        let matching = entities.iter().filter(|entity| state(entity)).collect::<Vec<_>>();
        let mut shown = matching
            .iter()
            .take(limit)
            .map(|entity| match (skip, entity.skip_kind.as_deref()) {
                (true, Some(kind)) => format!("{} ({})", entity.name, why(kind, None)),
                _ => entity.name.clone(),
            })
            .collect::<Vec<_>>();
        if matching.len() > limit {
            shown.push(format!("+{} more (--all)", matching.len() - limit));
        }
        shown.join(", ")
    };
    let mut groups = Vec::new();
    for (label, state, skip) in [
        ("ready", &(|entity: &EntityView| entity.state == "ready") as &dyn Fn(&EntityView) -> bool, false),
        (waiting, &|entity: &EntityView| !matches!(entity.state.as_str(), "ready" | "skipped"), false),
        ("skipped", &|entity: &EntityView| entity.state == "skipped", true),
    ] {
        let listed = names(state, skip);
        if !listed.is_empty() {
            groups.push(format!("{label}: {listed}"));
        }
    }
    groups.join(" · ")
}
