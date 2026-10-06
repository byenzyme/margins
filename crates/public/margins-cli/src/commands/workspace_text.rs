//! Plain-language output for people reading Workspace commands in a terminal.
//!
//! The wording follows the bb plugin's program editor and setup panel
//! (`integrations/bb-plugin-margins/src/workspace-program.ts` `plainSummaries`
//! and the setup preview in `meetings-page.tsx`), so the CLI and the panel
//! describe the same change the same way. `--json` output never passes
//! through here.

use margins_workflows::workspace::{
    SourceRole, WorkspaceApplyReceipt, WorkspaceBinding, WorkspaceConfig, WorkspaceEntityOptions,
    WorkspacePlan, WorkspacePlanAction, WorkspacePolicy, WorkspaceProgram,
};
use margins_workflows::workspace_program::derive_view;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// What a preset kept and skipped, as `workspace plan --preset` reports it.
#[derive(Debug, Clone, Default)]
pub struct PresetOutcome {
    pub template: String,
    pub skipped_readings: Vec<String>,
    pub skip_reasons: BTreeMap<String, String>,
}

/// `folder:People` → "the People folder"; `#x` → "notes tagged #x".
pub fn describe_ref(entity_ref: &str) -> String {
    if let Some(folder) = entity_ref.strip_prefix("folder:") {
        format!("the {folder} folder")
    } else if entity_ref.starts_with('#') {
        format!("notes tagged {entity_ref}")
    } else if let Some(tag) = entity_ref.strip_prefix("tag:") {
        format!("notes tagged #{tag}")
    } else if let Some(source) = entity_ref.strip_prefix("source:") {
        format!("the {source} source")
    } else {
        entity_ref.to_string()
    }
}

/// A reading as the setup panel lists it: `folder:People` → `People`.
pub fn reading_label(entity_ref: &str) -> &str {
    match entity_ref.get(..7) {
        Some(prefix) if prefix.eq_ignore_ascii_case("folder:") => &entity_ref[7..],
        _ => entity_ref,
    }
}

fn readings(policy: &WorkspacePolicy) -> Vec<(&str, Option<&WorkspaceEntityOptions>)> {
    policy
        .entities
        .iter()
        .flat_map(|entity| entity.entries())
        .collect()
}

fn same_options(a: Option<&WorkspaceEntityOptions>, b: Option<&WorkspaceEntityOptions>) -> bool {
    // As the plan JSON shows them: `children` is never serialized.
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.profile == b.profile && a.expandable == b.expandable,
        _ => false,
    }
}

/// Plain-language lines for one plan action. Anything not recognized keeps
/// the action's own summary; the diff stays exact.
pub fn plain_summaries(action: &WorkspacePlanAction) -> Vec<String> {
    let (before, after) = match action {
        WorkspacePlanAction::AddBinding { name, .. } => {
            return vec![format!("Add the source \"{name}\"")]
        }
        WorkspacePlanAction::RemoveBinding { name, .. } => {
            return vec![format!("Remove the source \"{name}\"")]
        }
        WorkspacePlanAction::UpdateBinding { name, .. } => {
            return vec![format!("Change the source \"{name}\"")]
        }
        WorkspacePlanAction::UpdateProgram { .. } => {
            return vec![
                "Other changes, such as learning settings, profiles, or layout (see the diff)"
                    .to_string(),
            ]
        }
        WorkspacePlanAction::SetPolicy { before, after, .. } => (before, after),
    };
    let mut lines = Vec::new();
    let was = readings(before);
    let now = readings(after);
    fn find<'a>(
        list: &[(&str, Option<&'a WorkspaceEntityOptions>)],
        wanted: &str,
    ) -> Option<Option<&'a WorkspaceEntityOptions>> {
        list.iter()
            .find(|(entity_ref, _)| *entity_ref == wanted)
            .map(|(_, options)| *options)
    }
    for (entity_ref, options) in &now {
        match find(&was, entity_ref) {
            None => lines.push(format!("Learn from {} (new)", describe_ref(entity_ref))),
            Some(previous) if !same_options(previous, *options) => lines.push(format!(
                "Change how Margins learns from {}",
                describe_ref(entity_ref)
            )),
            Some(_) => {}
        }
    }
    for (entity_ref, _) in &was {
        if find(&now, entity_ref).is_none() {
            lines.push(format!("Stop learning from {}", describe_ref(entity_ref)));
        }
    }
    let sets: [(&Vec<String>, &Vec<String>, fn(&str) -> String); 3] = [
        (&before.excluded_folders, &after.excluded_folders, |item| {
            format!("the {item} folder")
        }),
        (&before.excluded_tags, &after.excluded_tags, |item| {
            format!("notes tagged #{}", item.trim_start_matches('#'))
        }),
        (
            &before.excluded_entities,
            &after.excluded_entities,
            describe_ref,
        ),
    ];
    for (was, now, describe) in sets {
        for item in now.iter().filter(|item| !was.contains(item)) {
            lines.push(format!("Leave out {}", describe(item)));
        }
        for item in was.iter().filter(|item| !now.contains(item)) {
            lines.push(format!("Stop leaving out {}", describe(item)));
        }
    }
    if lines.is_empty() {
        lines.push(action.summary().to_string());
    }
    lines
}

/// "Your Workspace is the program at …", with how to read and change it.
pub fn write_program_block(out: &mut dyn Write, id: &str, program_path: &Path) -> io::Result<()> {
    writeln!(
        out,
        "Your Workspace is the program at {}",
        program_path.display()
    )?;
    writeln!(
        out,
        "  Read it:   margins --workspace {id} workspace show --text"
    )?;
    writeln!(out, "  Change it: margins --workspace {id} workspace edit")
}

/// The desired program's view, for what Margins will do once it is applied.
fn desired_view(plan: &WorkspacePlan, current: &WorkspaceConfig) -> Option<WorkspaceConfig> {
    let program = WorkspaceProgram::parse(&plan.desired_program).ok()?;
    derive_view(&program, current.name.clone(), current.retention.clone()).ok()
}

fn note_destination(view: &WorkspaceConfig) -> Option<PathBuf> {
    view.bindings.values().find_map(|binding| match binding {
        WorkspaceBinding::NativeMarkdown {
            path,
            role: SourceRole::Home,
            note_folder,
        } => Some(match note_folder {
            Some(folder) if folder != Path::new(".") => path.join(folder),
            _ => path.clone(),
        }),
        _ => None,
    })
}

/// Where a human-mode plan is saved for `workspace apply --plan`: the system
/// temp directory, named by Workspace and plan id, so planning the same change
/// twice reuses one file. Nothing is written into the Margins home.
pub fn plan_file_path(workspace_id: &str, plan_id: &str) -> PathBuf {
    let short = plan_id.get(..12).unwrap_or(plan_id);
    std::env::temp_dir().join(format!("margins-{workspace_id}-plan-{short}.json"))
}

/// The consequences of a plan in plain language, the exact diff, and how to
/// apply it. `saved` is the plan file (`None` when there is nothing to apply).
pub fn write_plan(
    out: &mut dyn Write,
    current: &WorkspaceConfig,
    program_path: &Path,
    plan: &WorkspacePlan,
    preset: Option<&PresetOutcome>,
    saved: Option<&Path>,
) -> io::Result<()> {
    let id = &plan.workspace_id;
    writeln!(out, "Workspace {id}: plan for {}", program_path.display())?;
    writeln!(out)?;
    if plan.actions.is_empty() && plan.diff.is_empty() {
        writeln!(out, "No changes: the program already says this.")?;
    } else {
        writeln!(out, "Changes:")?;
        for action in &plan.actions {
            for summary in plain_summaries(action) {
                writeln!(out, "  • {summary}")?;
            }
        }
    }
    writeln!(out)?;
    writeln!(out, "Once applied, Margins:")?;
    if let Some(view) = desired_view(plan, current) {
        let kept = readings(&view.policy)
            .into_iter()
            .map(|(entity_ref, _)| reading_label(entity_ref).to_string())
            .collect::<Vec<_>>();
        if kept.is_empty() {
            writeln!(out, "  Chooses what to learn from automatically")?;
        } else {
            writeln!(out, "  Learns from {}", kept.join(" · "))?;
            if view.policy.automatic.is_some() {
                writeln!(out, "  …and picks what those readings miss automatically")?;
            }
        }
        let left_out = view
            .policy
            .excluded_folders
            .iter()
            .cloned()
            .chain(
                view.policy
                    .excluded_tags
                    .iter()
                    .map(|tag| format!("#{}", tag.trim_start_matches('#'))),
            )
            .chain(
                view.policy
                    .excluded_entities
                    .iter()
                    .map(|entity_ref| reading_label(entity_ref).to_string()),
            )
            .collect::<Vec<_>>();
        if left_out.is_empty() {
            writeln!(out, "  Leaves nothing out")?;
        } else {
            writeln!(out, "  Leaves out {}", left_out.join(" · "))?;
        }
        if let Some(destination) = note_destination(&view) {
            writeln!(out, "  Notes will go to {}", destination.display())?;
        }
    }
    if let Some(preset) = preset {
        if !preset.skipped_readings.is_empty() {
            let skipped = preset
                .skipped_readings
                .iter()
                .map(|reading| match preset.skip_reasons.get(reading) {
                    Some(reason) => format!("{} ({reason})", reading_label(reading)),
                    None => reading_label(reading).to_string(),
                })
                .collect::<Vec<_>>();
            writeln!(out, "  Skipped, not in your notes: {}", skipped.join(" · "))?;
        }
        writeln!(out, "  (started from the {} preset)", preset.template)?;
    }
    if !plan.diff.is_empty() {
        writeln!(out)?;
        writeln!(out, "Exact change to the program:")?;
        write!(out, "{}", plan.diff)?;
        if !plan.diff.ends_with('\n') {
            writeln!(out)?;
        }
    }
    writeln!(out)?;
    match saved {
        Some(saved) => {
            writeln!(out, "Nothing is applied yet. To apply exactly this plan:")?;
            writeln!(
                out,
                "  margins --workspace {id} workspace apply --plan {}",
                saved.display()
            )
        }
        None => writeln!(out, "Nothing to apply."),
    }
}

/// What an applied plan changed, then the program block.
pub fn write_applied(
    out: &mut dyn Write,
    receipt: &WorkspaceApplyReceipt,
    program_path: &Path,
) -> io::Result<()> {
    let id = &receipt.workspace_id;
    let revision = receipt
        .after_revision
        .get(..12)
        .unwrap_or(&receipt.after_revision);
    if receipt.replayed {
        writeln!(
            out,
            "This plan was already applied; Workspace {id} is unchanged (revision {revision})."
        )?;
    } else if receipt.actions.is_empty() {
        writeln!(
            out,
            "Nothing changed in Workspace {id} (revision {revision})."
        )?;
    } else {
        writeln!(out, "Applied to Workspace {id} (revision {revision}):")?;
        for action in &receipt.actions {
            for summary in plain_summaries(&action.action) {
                writeln!(out, "  • {summary}")?;
            }
        }
    }
    writeln!(out)?;
    write_program_block(out, id, program_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_workflows::workspace::WorkspaceEntity;

    fn options(profile: &str) -> WorkspaceEntityOptions {
        WorkspaceEntityOptions {
            profile: Some(profile.to_string()),
            ..Default::default()
        }
    }

    /// The same cases as the plugin's `plainSummaries` test, so the CLI and
    /// the program editor keep saying the same thing.
    #[test]
    fn plain_summaries_match_the_program_editor() {
        let action = WorkspacePlanAction::SetPolicy {
            summary: "Attention policy: …".to_string(),
            before: WorkspacePolicy {
                entities: vec![
                    WorkspaceEntity::simple("#old"),
                    WorkspaceEntity::with_options("folder:Meetings", options("operational")),
                ],
                excluded_folders: vec!["Archive".to_string()],
                ..Default::default()
            },
            after: WorkspacePolicy {
                entities: vec![
                    WorkspaceEntity::with_options("folder:Meetings", options("decisions")),
                    WorkspaceEntity::with_options("folder:Projects", options("decisions")),
                ],
                excluded_folders: vec!["Templates".to_string()],
                excluded_tags: vec!["private".to_string()],
                ..Default::default()
            },
        };
        assert_eq!(
            plain_summaries(&action),
            [
                "Change how Margins learns from the Meetings folder",
                "Learn from the Projects folder (new)",
                "Stop learning from notes tagged #old",
                "Leave out the Templates folder",
                "Stop leaving out the Archive folder",
                "Leave out notes tagged #private",
            ]
        );
        let binding = WorkspaceBinding::Captures {
            path: PathBuf::from("/c"),
        };
        assert_eq!(
            plain_summaries(&WorkspacePlanAction::AddBinding {
                summary: "x".into(),
                name: "chat".into(),
                binding,
            }),
            ["Add the source \"chat\""]
        );
        assert!(plain_summaries(&WorkspacePlanAction::UpdateProgram {
            summary: "x".into()
        })[0]
            .contains("see the diff"));
        let same = WorkspacePolicy {
            entities: vec![WorkspaceEntity::simple("#a")],
            ..Default::default()
        };
        assert_eq!(
            plain_summaries(&WorkspacePlanAction::SetPolicy {
                summary: "Attention policy: reorder".into(),
                before: same.clone(),
                after: same,
            }),
            ["Attention policy: reorder"]
        );
    }

    #[test]
    fn program_block_names_the_program_and_how_to_read_and_change_it() {
        let mut out = Vec::new();
        write_program_block(
            &mut out,
            "practice",
            Path::new("/m/configs/practice.enzyme"),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Your Workspace is the program at /m/configs/practice.enzyme\n  \
             Read it:   margins --workspace practice workspace show --text\n  \
             Change it: margins --workspace practice workspace edit\n"
        );
    }
}
