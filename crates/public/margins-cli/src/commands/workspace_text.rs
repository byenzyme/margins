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

const ADDED: &str = "\x1b[32m";
const REMOVED: &str = "\x1b[31m";
const RESET: &str = "\x1b[0m";

/// A unified diff, ending in a newline. With `color`, added lines are green
/// and removed lines red; the `---`/`+++` file headers stay plain. Without
/// it, the diff is written byte for byte.
pub fn write_diff(out: &mut dyn Write, diff: &str, color: bool) -> io::Result<()> {
    if !color {
        write!(out, "{diff}")?;
    } else {
        for line in diff.split_inclusive('\n') {
            let (body, newline) = match line.strip_suffix('\n') {
                Some(body) => (body, "\n"),
                None => (line, ""),
            };
            let paint = if body.starts_with("+++") || body.starts_with("---") {
                None
            } else if body.starts_with('+') {
                Some(ADDED)
            } else if body.starts_with('-') {
                Some(REMOVED)
            } else {
                None
            };
            match paint {
                Some(paint) => write!(out, "{paint}{body}{RESET}{newline}")?,
                None => write!(out, "{line}")?,
            }
        }
    }
    if !diff.is_empty() && !diff.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(())
}

/// Whether `id` is the machine default Workspace (false when that cannot be
/// read), so printed commands can leave out `--workspace`.
pub fn is_machine_default(id: &str) -> bool {
    margins_workflows::workspace::margins_home()
        .and_then(|home| margins_workflows::workspace::default_workspace(&home))
        .is_ok_and(|default| default.as_deref() == Some(id))
}

/// The start of a printed command for Workspace `id`: plain `margins` when
/// that alone selects `id` from here (`MARGINS_WORKSPACE`, else the Workspace
/// covering the current folder, else the default), otherwise with
/// `--workspace <id>`.
pub fn margins_for(id: &str, is_default: bool) -> String {
    let cwd = std::env::current_dir().ok();
    if plain_selects(id, is_default, cwd.as_deref()) {
        "margins".to_string()
    } else {
        format!("margins --workspace {id}")
    }
}

fn plain_selects(id: &str, is_default: bool, cwd: Option<&Path>) -> bool {
    if let Some(selected) = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return selected.trim() == id;
    }
    let covering = cwd.and_then(|cwd| {
        margins_workflows::workspace::margins_home()
            .and_then(|home| margins_workflows::workspace::covering_workspace_id(&home, cwd))
            .ok()
    });
    match covering {
        Some(Some(covering)) => covering == id,
        // No Workspace covers the folder: the default is what plain selects.
        Some(None) => is_default,
        // Unreadable, or several cover it: spell the Workspace out.
        None => false,
    }
}

/// "Your Workspace is the program at …", with how to read and change it.
pub fn write_program_block(
    out: &mut dyn Write,
    id: &str,
    program_path: &Path,
    is_default: bool,
) -> io::Result<()> {
    let margins = margins_for(id, is_default);
    writeln!(
        out,
        "Your Workspace is the program at {}",
        program_path.display()
    )?;
    writeln!(out, "  See what it learns: {margins} status")?;
    writeln!(out, "  Change it:          {margins} edit")
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

/// The private directory human-mode plans are saved in: `$MARGINS_HOME/plans`.
pub const PLANS_DIR: &str = "plans";

/// Saved plans kept: at most this many, newest first ...
pub const MAX_KEPT_PLANS: usize = 40;
/// ... and none older than this. A plan this old is almost always stale; its
/// apply would be refused anyway once the program has changed.
pub const PLAN_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Remove saved plans beyond [`MAX_KEPT_PLANS`] or older than
/// [`PLAN_MAX_AGE`], like the bb plugin prunes its own. Only regular
/// `*.plan.json` files are considered (never symlinks or directories), and
/// pruning is best-effort: a file that cannot be read or removed is skipped.
pub fn prune_plans(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut plans = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".plan.json"))
        .filter_map(|entry| {
            let metadata = std::fs::symlink_metadata(entry.path()).ok()?;
            metadata
                .file_type()
                .is_file()
                .then(|| (entry.path(), metadata.modified().unwrap_or(now)))
        })
        .collect::<Vec<_>>();
    plans.sort_by(|a, b| b.1.cmp(&a.1));
    for (index, (path, modified)) in plans.into_iter().enumerate() {
        let age = now.duration_since(modified).unwrap_or_default();
        // Room for the plan about to be saved.
        if index + 1 >= MAX_KEPT_PLANS || age > PLAN_MAX_AGE {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Save a human-mode plan for `workspace apply --plan` in
/// `$MARGINS_HOME/plans/` (mode 0700), as a new file with a random name and
/// mode 0600. A `plans` that is a symlink or not a directory is refused, and
/// the file is created exclusively, so no existing path is ever followed or
/// overwritten.
pub fn save_plan(margins_home: &Path, workspace_id: &str, plan_json: &[u8]) -> io::Result<PathBuf> {
    let dir = margins_home.join(PLANS_DIR);
    match std::fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} is not a private directory", dir.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(&dir)?;
        }
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    prune_plans(&dir);
    // `tempfile` creates with O_CREAT|O_EXCL (never through a symlink) and 0600.
    let mut file = tempfile::Builder::new()
        .prefix(&format!("{workspace_id}-"))
        .suffix(".plan.json")
        .rand_bytes(12)
        .tempfile_in(&dir)?;
    file.write_all(plan_json)?;
    file.as_file().sync_all()?;
    file.keep()
        .map(|(_, path)| path)
        .map_err(|error| error.error)
}

/// "Once applied, Margins: learns from … leaves out … notes go to …": the
/// effect of a plan on what Margins reads, shown by `workspace plan` and
/// before `edit` asks to apply.
pub fn write_effect(out: &mut dyn Write, current: &WorkspaceConfig, plan: &WorkspacePlan) -> io::Result<()> {
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
    Ok(())
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
    color: bool,
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
    write_effect(out, current, plan)?;
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
        write_diff(out, &plan.diff, color)?;
    }
    writeln!(out)?;
    match saved {
        Some(saved) => {
            writeln!(out, "Nothing is applied yet. To apply exactly this plan:")?;
            writeln!(out, "  margins workspace apply --plan {}", saved.display())
        }
        None => writeln!(out, "Nothing to apply."),
    }
}

/// What an applied plan changed, then the program block.
pub fn write_applied(
    out: &mut dyn Write,
    receipt: &WorkspaceApplyReceipt,
    program_path: &Path,
    is_default: bool,
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
    write_program_block(out, id, program_path, is_default)
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
        let block = |is_default| {
            let mut out = Vec::new();
            write_program_block(
                &mut out,
                "practice",
                Path::new("/m/configs/practice.enzyme"),
                is_default,
            )
            .unwrap();
            String::from_utf8(out).unwrap()
        };
        assert_eq!(
            block(false),
            "Your Workspace is the program at /m/configs/practice.enzyme\n  \
             See what it learns: margins --workspace practice status\n  \
             Change it:          margins --workspace practice edit\n"
        );
        // The machine default needs no selector.
        assert_eq!(
            block(true),
            "Your Workspace is the program at /m/configs/practice.enzyme\n  \
             See what it learns: margins status\n  \
             Change it:          margins edit\n"
        );
    }

    #[test]
    fn diff_colours_added_and_removed_lines_but_not_headers() {
        let diff = "--- a/x.enzyme\n+++ b/x.enzyme\n@@ -1 +1 @@\n-old\n+new\n same";
        let mut plain = Vec::new();
        write_diff(&mut plain, diff, false).unwrap();
        assert_eq!(String::from_utf8(plain).unwrap(), format!("{diff}\n"));
        let mut coloured = Vec::new();
        write_diff(&mut coloured, diff, true).unwrap();
        assert_eq!(
            String::from_utf8(coloured).unwrap(),
            "--- a/x.enzyme\n+++ b/x.enzyme\n@@ -1 +1 @@\n\x1b[31m-old\x1b[0m\n\x1b[32m+new\x1b[0m\n same\n"
        );
    }
}
