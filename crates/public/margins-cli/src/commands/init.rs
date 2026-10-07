//! `margins init [PATH]`: what it acts on, and the receipt it prints.
//!
//! Init either refreshes a Workspace that already covers the folder (or the
//! one selected with `--workspace`), or makes a new one for the folder. It
//! never re-applies the preset to an existing Workspace: the preset adds to
//! a program, and would fight the person's own edits. The official
//! composition indexes and offers catalysts; this module only decides the
//! target and renders the outcome, so both compositions say the same thing.

use super::workspace_text::{self, reading_label};
use crate::error::CliError;
use margins_workflows::workspace::{self, ImplicitWorkspaceRoots, ResolvedWorkspace};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const INIT_SCHEMA: &str = "margins.init.v1";

/// What `margins init` acts on.
#[derive(Debug)]
pub enum InitTarget {
    /// A Workspace that exists: selected, or covering the folder.
    Existing {
        workspace: Box<ResolvedWorkspace>,
        /// `None` when selected; else the Source that covers the folder.
        covered_by: Option<String>,
    },
    /// A new Workspace for `folder`.
    New {
        folder: PathBuf,
        id: String,
        /// Allowances an explicitly named folder received (temporary, empty).
        notes: Vec<String>,
    },
}

/// Decide what `margins init` acts on. Writes nothing.
pub fn target(
    selector: Option<&str>,
    path: Option<&Path>,
    id: Option<&str>,
    cwd: &Path,
) -> Result<InitTarget, CliError> {
    let margins_home = workspace::margins_home().map_err(CliError::from_anyhow)?;
    let env_selector = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let selected = selector
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or(env_selector);
    if let Some(selected) = selected {
        if path.is_some() || id.is_some() {
            return Err(CliError::usage(
                "`margins init` takes either --workspace <id> (refresh that Workspace) or a folder and --id (make a new one), not both",
            ));
        }
        let workspace = workspace::resolve_at(&margins_home, &selected).map_err(|error| {
            CliError::new(
                "workspace_not_found",
                format!(
                    "{error:#}. To make it, run `margins init /path/to/notes --id {selected}`."
                ),
            )
        })?;
        return Ok(InitTarget::Existing {
            workspace: Box::new(workspace),
            covered_by: None,
        });
    }

    let explicit = path.is_some();
    let folder = match path {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => cwd.join(path),
        None => cwd.to_path_buf(),
    };
    let folder = folder.canonicalize().map_err(|error| {
        CliError::new(
            "folder_not_found",
            format!("{}: {error}", folder.display()),
        )
    })?;
    let covering = workspace::inspect_workspace_entries(&margins_home)
        .map_err(CliError::from_anyhow)?
        .into_iter()
        .filter_map(|(_, resolved)| resolved.ok())
        .filter_map(|candidate| {
            let source = candidate
                .config
                .bindings
                .iter()
                .find(|(_, binding)| {
                    binding
                        .local_path()
                        .is_some_and(|path| folder.starts_with(path))
                })
                .map(|(name, _)| name.clone())
                .or_else(|| folder.starts_with(&candidate.home_dir).then(|| "home".to_string()))?;
            Some((candidate, source))
        })
        .collect::<Vec<_>>();
    match covering.len() {
        0 => {}
        1 => {
            let (candidate, source) = covering.into_iter().next().expect("one Workspace");
            if let Some(id) = id.filter(|id| *id != candidate.config.id) {
                return Err(CliError::new(
                    "folder_already_in_workspace",
                    format!(
                        "{} is already part of Workspace {}, so it cannot also start Workspace {id}",
                        folder.display(),
                        candidate.config.id
                    ),
                ));
            }
            // Resolve for writing: init may migrate a retired layout.
            let workspace = workspace::resolve_at(&margins_home, &candidate.config.id)
                .map_err(CliError::from_anyhow)?;
            return Ok(InitTarget::Existing {
                workspace: Box::new(workspace),
                covered_by: Some(source),
            });
        }
        _ => {
            let ids = covering
                .iter()
                .map(|(candidate, _)| candidate.config.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CliError::new(
                "workspace_ambiguous",
                format!(
                    "{} is part of several Workspaces ({ids}); pick one with `margins --workspace <id> init`",
                    folder.display()
                ),
            ));
        }
    }

    let roots = ImplicitWorkspaceRoots::from_process(&margins_home).map_err(CliError::from_anyhow)?;
    let notes = workspace::check_new_workspace_folder(&roots, &folder, explicit).map_err(|error| {
        CliError::new("folder_refused", format!("{error:#}"))
    })?;
    let id = match id {
        Some(id) => {
            if workspace::workspace_program_path(&margins_home, id)
                .map_err(|error| CliError::new("invalid_workspace_id", format!("{error:#}")))?
                .exists()
                || workspace::workspace_state_dir(&margins_home, id)
                    .map_err(CliError::from_anyhow)?
                    .exists()
            {
                return Err(CliError::new(
                    "workspace_exists",
                    format!("Workspace {id} already exists for other notes; choose another --id"),
                ));
            }
            if workspace::is_reserved_id(id) {
                return Err(CliError::new(
                    "invalid_workspace_id",
                    format!("{:#}", workspace::reserved_id_error(id)),
                ));
            }
            id.to_string()
        }
        None => workspace::new_workspace_id(&margins_home, &folder).map_err(CliError::from_anyhow)?,
    };
    Ok(InitTarget::New { folder, id, notes })
}

/// The `margins.init.v1` receipt, and what the readable output says.
#[derive(Debug, Clone, Serialize)]
pub struct InitReceipt {
    pub schema_version: &'static str,
    pub workspace: InitWorkspace,
    pub created: bool,
    /// The Source covering the folder, when an existing Workspace was refreshed.
    pub covered_by: Option<String>,
    pub default_set: bool,
    pub preset: Option<InitPreset>,
    pub recall: InitRecall,
    /// What the program asks Margins to read, in plain words.
    pub reads: Vec<String>,
    /// What the engine selected after indexing and does not skip, in plain
    /// words; `None` without an engine index (public build).
    pub learns_about: Option<Vec<String>>,
    /// Selected but skipped, with the reason in plain words.
    pub skipped: Vec<InitSkip>,
    /// What the program leaves out.
    pub leaves_out: Vec<String>,
    /// Changes in attention since the previous index refresh, when known.
    pub attention: Option<serde_json::Value>,
    /// Allowances for an explicitly named folder (temporary, empty).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitSkip {
    pub what: String,
    pub why: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitWorkspace {
    pub id: String,
    pub home: String,
    pub program: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitPreset {
    pub template: String,
    pub readings: Vec<String>,
    pub skipped_readings: Vec<String>,
    pub skip_reasons: std::collections::BTreeMap<String, String>,
    pub note_folder: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InitRecall {
    /// `ok` (index and catalysts), `index_only` (no catalyst generator),
    /// `catalysts_pending` (generation incomplete), or `lexical` (public
    /// build without an index).
    pub status: String,
    pub documents: usize,
    pub catalysts: usize,
    /// `hosted`, `local`, or `none`.
    pub catalyst_mode: String,
    /// The one command that turns catalysts on or finishes them, when needed.
    pub next_step: Option<String>,
}

impl InitReceipt {
    pub fn workspace_view(workspace: &ResolvedWorkspace) -> InitWorkspace {
        InitWorkspace {
            id: workspace.config.id.clone(),
            home: workspace.home_dir.display().to_string(),
            program: workspace.config_path.display().to_string(),
            is_default: workspace_text::is_machine_default(&workspace.config.id),
        }
    }
}

/// The readings a Workspace program declares, as people read them.
pub fn declared_readings(workspace: &ResolvedWorkspace) -> Vec<String> {
    workspace
        .config
        .policy
        .entities
        .iter()
        .flat_map(|entity| {
            entity
                .entries()
                .into_iter()
                .map(|(entity_ref, options)| {
                    let mut label = workspace_text::describe_ref(entity_ref);
                    if options.is_some_and(|options| options.expandable) {
                        label.push_str(" and the pages it links");
                    }
                    if let Some(profile) = options.and_then(|options| options.profile.as_deref()) {
                        label.push_str(&format!(" — {profile}"));
                    }
                    label
                })
                .collect::<Vec<_>>()
        })
        .chain(workspace.config.policy.automatic.is_some().then(|| {
            if workspace.config.policy.entities.is_empty() {
                "what it picks automatically".to_string()
            } else {
                "…plus what those miss, picked automatically".to_string()
            }
        }))
        .collect()
}

/// What the program leaves out, in plain words.
pub fn left_out(workspace: &ResolvedWorkspace) -> Vec<String> {
    let policy = &workspace.config.policy;
    policy
        .excluded_folders
        .iter()
        .cloned()
        .chain(policy.excluded_tags.iter().map(|tag| format!("#{}", tag.trim_start_matches('#'))))
        .chain(policy.excluded_entities.iter().map(|entity| reading_label(entity).to_string()))
        .collect()
}

pub fn write(receipt: &InitReceipt, json: bool, out: &mut dyn Write) -> Result<(), CliError> {
    let output = |error: std::io::Error| CliError::new("output_failed", error.to_string());
    if json {
        serde_json::to_writer_pretty(&mut *out, receipt)
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        return writeln!(out).map_err(output);
    }
    write_text(receipt, out).map_err(output)
}

/// A reading's identity for matching "Reads" against "Skipped": its plain name
/// without how it is read (`… and the pages it links`, `— profile`).
fn reading_key(label: &str) -> String {
    let base = label.split(" and the pages it links").next().unwrap_or(label);
    let base = base.split(" — ").next().unwrap_or(base).trim();
    base.strip_prefix("notes tagged ")
        .unwrap_or(base)
        .to_lowercase()
}

fn write_text(receipt: &InitReceipt, out: &mut dyn Write) -> std::io::Result<()> {
    let ws = &receipt.workspace;
    let margins = workspace_text::margins_for(&ws.id, ws.is_default);
    for note in &receipt.notes {
        writeln!(out, "Note: {note}")?;
    }
    if receipt.created {
        writeln!(
            out,
            "Created Workspace {} for {}{}",
            ws.id,
            ws.home,
            if receipt.default_set { " (now your default)" } else { "" }
        )?;
    } else {
        match &receipt.covered_by {
            Some(source) if source != "home" => writeln!(
                out,
                "Already part of Workspace {} (Source {source}; new notes still go to its home, {})",
                ws.id, ws.home
            )?,
            Some(_) => writeln!(out, "Already part of Workspace {} ({})", ws.id, ws.home)?,
            None => writeln!(out, "Workspace {} ({})", ws.id, ws.home)?,
        }
    }
    // A reading the engine skipped is shown once, under Skipped, with why.
    let skipped_whats = receipt
        .skipped
        .iter()
        .map(|skip| reading_key(&skip.what))
        .collect::<std::collections::BTreeSet<_>>();
    let reads = receipt
        .reads
        .iter()
        .filter(|read| !skipped_whats.contains(&reading_key(read)))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !reads.is_empty() {
        writeln!(out, "Reads: {}", reads.join(" · "))?;
    }
    if let Some(learns) = &receipt.learns_about {
        writeln!(
            out,
            "Learns about now: {}",
            if learns.is_empty() {
                "nothing yet".to_string()
            } else {
                learns.join(", ")
            }
        )?;
    }
    // One list: what the engine skipped, and preset folders the notes lack.
    let mut skipped = receipt
        .skipped
        .iter()
        .map(|skip| format!("{} ({})", skip.what, skip.why))
        .collect::<Vec<_>>();
    if let Some(preset) = &receipt.preset {
        skipped.extend(preset.skipped_readings.iter().map(|reading| {
            let why = match preset.skip_reasons.get(reading).map(String::as_str) {
                None | Some("no such folder") => "not in your notes",
                Some(reason) => reason,
            };
            format!("{} ({why})", workspace_text::describe_ref(reading))
        }));
    }
    if !skipped.is_empty() {
        writeln!(out, "Skipped: {}", skipped.join("; "))?;
    }
    writeln!(
        out,
        "Leaves out: {}",
        if receipt.leaves_out.is_empty() {
            "nothing".to_string()
        } else {
            receipt.leaves_out.join(" · ")
        }
    )?;
    if let Some(preset) = &receipt.preset {
        let folder = if preset.note_folder == "." {
            ws.home.clone()
        } else {
            format!("{}/{}", ws.home.trim_end_matches('/'), preset.note_folder)
        };
        writeln!(out, "New notes go to: {folder}")?;
    }
    if let Some(attention) = &receipt.attention {
        if let Some(line) = attention.get("summary").and_then(serde_json::Value::as_str) {
            writeln!(out, "Since last time: {line}")?;
        }
    }
    let recall = &receipt.recall;
    let line = match recall.status.as_str() {
        "ok" => format!(
            "Recall: {} indexed · {} {} ready",
            notes(recall.documents),
            recall.catalysts,
            if recall.catalysts == 1 { "catalyst" } else { "catalysts" }
        ),
        "catalysts_pending" => format!(
            "Recall: {} indexed · catalysts still building ({} ready)",
            notes(recall.documents),
            recall.catalysts
        ),
        "lexical" => format!(
            "Recall: {} searchable directly (this build has no catalysts)",
            notes(recall.documents)
        ),
        _ => format!(
            "Recall: {} indexed · catalysts are off",
            notes(recall.documents)
        ),
    };
    writeln!(out, "{line}")?;
    if let Some(step) = &recall.next_step {
        writeln!(out, "  {step}")?;
    }
    writeln!(
        out,
        "See what it learned: `{margins} status` · Change it: `{margins} edit`"
    )
}

fn notes(count: usize) -> String {
    format!("{count} {}", if count == 1 { "note" } else { "notes" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skipped_reading_is_listed_once_under_skipped() {
        let receipt = InitReceipt {
            schema_version: INIT_SCHEMA,
            workspace: InitWorkspace {
                id: "notes".into(),
                home: "/notes".into(),
                program: "/state/configs/notes.enzyme".into(),
                is_default: true,
            },
            created: true,
            covered_by: None,
            default_set: false,
            preset: None,
            recall: InitRecall {
                status: "ok".into(),
                documents: 3,
                catalysts: 2,
                catalyst_mode: "hosted".into(),
                next_step: None,
            },
            reads: vec![
                "the Meetings folder".into(),
                "the People folder and the pages it links".into(),
                "…plus what those miss, picked automatically".into(),
            ],
            learns_about: Some(vec!["the Meetings folder".into()]),
            skipped: vec![InitSkip {
                what: "the People folder".into(),
                why: "too few notes".into(),
            }],
            leaves_out: Vec::new(),
            attention: None,
            notes: Vec::new(),
        };
        let mut out = Vec::new();
        write(&receipt, false, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let reads = text.lines().find(|line| line.starts_with("Reads: ")).unwrap();
        assert!(!reads.contains("People"), "{text}");
        assert!(reads.contains("the Meetings folder"), "{text}");
        assert!(text.contains("Skipped: the People folder (too few notes)"), "{text}");
        assert_eq!(text.matches("People").count(), 1, "{text}");
    }
}
