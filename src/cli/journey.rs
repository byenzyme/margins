// The journey commands people run: `init`, `status`, and `edit`'s preview,
// composed with the engine. Included into `cli.rs`.

/// Test/harness-only: treat `margins init` as interactive, so the catalyst
/// choice can be answered on piped stdin.
#[cfg(feature = "recall")]
const INIT_ASSUME_TERMINAL_ENV: &str = "MARGINS_INIT_ASSUME_TERMINAL";

/// `margins init [PATH] [--id ID] [--no-preset] [--json]`.
///
/// A folder no Workspace covers gets a new Workspace from the
/// margins-meetings preset, indexed now: search works with no download.
/// Catalysts are offered once, inline, when none is set up and a person is at
/// the terminal; otherwise init indexes only and names the command that turns
/// them on. The preset is filled and checked before anything is written, and
/// a Workspace whose first index fails is removed again, so a failed init
/// leaves nothing half-made. A folder that is already part of a Workspace (or
/// `--workspace ID`) is refreshed without touching its program.
#[cfg(feature = "recall")]
fn run_init_journey(
    workspace_selector: Option<&str>,
    path: Option<&Path>,
    id: Option<&str>,
    no_preset: bool,
    json: bool,
) -> i32 {
    use margins_cli::commands::init::{self as init, InitTarget};
    let fail = |error: margins_cli::CliError| report_for(json, error);
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let margins_home = match margins_workflows::workspace::margins_home() {
        Ok(home) => home,
        Err(error) => return fail(margins_cli::CliError::from_anyhow(error)),
    };
    let target = match init::target(workspace_selector, path, id, &cwd) {
        Ok(target) => target,
        Err(error) => return fail(error),
    };
    match target {
        InitTarget::Existing {
            workspace,
            covered_by,
        } => {
            if !json {
                eprintln!("Refreshing Workspace {}…", workspace.config.id);
            }
            let status = match crate::recall::provision_workspace_for_init(&workspace) {
                Ok(status) => status,
                Err(error) => {
                    return fail(margins_cli::CliError::new(
                        "index_failed",
                        margins_user_message(&format!("indexing your notes: {error:#}")),
                    ))
                }
            };
            finish_init(&margins_home, &workspace, false, covered_by, false, None, Vec::new(), &status, json)
        }
        InitTarget::New { folder, id, notes } => {
            if !json {
                for note in &notes {
                    eprintln!("Note: {note}");
                }
            }
            offer_catalysts(&margins_home, json);
            // Fill and check the preset before anything is written.
            let mut config =
                match margins_workflows::workspace::new_workspace_config(&margins_home, &id, &folder) {
                    Ok(config) => config,
                    Err(error) => return fail(margins_cli::CliError::from_anyhow(error)),
                };
            let preset = if no_preset {
                None
            } else {
                match fill_meetings_preset(&margins_home, &config, &folder) {
                    Ok(proposal) => {
                        config = proposal.desired.clone();
                        Some(proposal)
                    }
                    Err(error) => return fail(error),
                }
            };
            let workspace = match margins_workflows::workspace::create_workspace_with_config(
                &margins_home,
                &config,
                None,
            ) {
                Ok(workspace) => workspace,
                Err(error) => return fail(margins_cli::CliError::from_anyhow(error)),
            };
            let rollback = |error: margins_cli::CliError| {
                if let Err(cleanup) =
                    margins_workflows::workspace::discard_new_workspace(&margins_home, &id)
                {
                    eprintln!("Could not remove the unfinished Workspace {id}: {cleanup:#}");
                }
                fail(error)
            };
            let default_set = match margins_workflows::workspace::default_workspace(&margins_home) {
                Ok(None) => match margins_workflows::workspace::set_default_workspace(&margins_home, &id) {
                    Ok(()) => true,
                    Err(error) => return rollback(margins_cli::CliError::from_anyhow(error)),
                },
                Ok(Some(_)) => false,
                Err(error) => return rollback(margins_cli::CliError::from_anyhow(error)),
            };
            if !json {
                eprintln!("Indexing your notes in {}…", folder.display());
            }
            let status = match crate::recall::provision_workspace_for_init(&workspace) {
                Ok(status) => status,
                Err(error) => {
                    return rollback(margins_cli::CliError::new(
                        "index_failed",
                        margins_user_message(&format!(
                            "indexing your notes failed, so no Workspace was made: {error:#}"
                        )),
                    ))
                }
            };
            let preset = preset.map(|proposal| init::InitPreset {
                template: margins_workflows::workspace_preset::MEETINGS_PRESET.to_string(),
                readings: proposal.readings,
                skipped_readings: proposal.skipped_readings,
                skip_reasons: proposal.skip_reasons,
                note_folder: proposal.note_folder,
            });
            finish_init(&margins_home, &workspace, true, None, default_set, preset, notes, &status, json)
        }
    }
}

#[cfg(feature = "recall")]
#[allow(clippy::too_many_arguments)]
fn finish_init(
    margins_home: &Path,
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    created: bool,
    covered_by: Option<String>,
    default_set: bool,
    preset: Option<margins_cli::commands::init::InitPreset>,
    notes: Vec<String>,
    status: &crate::recall::InitStatus,
    json: bool,
) -> i32 {
    use margins_cli::commands::init::{self as init, InitReceipt, InitRecall};
    let catalyst = margins_workflows::catalyst::selected_status(margins_home);
    let mut learns_about = init::declared_readings(workspace);
    if workspace.config.policy.automatic.is_some() {
        learns_about.push(if learns_about.is_empty() {
            "whatever it picks automatically".to_string()
        } else {
            "…plus what those miss, picked automatically".to_string()
        });
    }
    let next_step = match status.status {
        "index_only" => Some(crate::recall::ENABLE_CATALYSTS_HINT.to_string()),
        "catalysts_pending" => Some(
            "Catalysts did not finish building; run `margins sync` to finish them.".to_string(),
        ),
        _ => None,
    };
    let attention = status.attention.as_ref().map(|diff| {
        let mut value = serde_json::to_value(diff).unwrap_or(serde_json::Value::Null);
        // A first build has no "since last time".
        if diff.baseline {
            value["summary"] = serde_json::Value::Null;
        }
        value
    });
    let receipt = InitReceipt {
        schema_version: init::INIT_SCHEMA,
        workspace: InitReceipt::workspace_view(workspace),
        created,
        covered_by,
        default_set,
        preset,
        recall: InitRecall {
            status: status.status.to_string(),
            documents: status.documents,
            catalysts: status.catalysts,
            catalyst_mode: catalyst.mode.as_str().to_string(),
            next_step,
        },
        learns_about,
        attention,
        notes: if json { notes } else { Vec::new() },
    };
    if let Err(error) = init::write(&receipt, json, &mut io::stdout()) {
        return report_for(json, error);
    }
    // Catalysts the chosen generator failed to build are a failure to report,
    // but the Workspace is indexed and kept: `margins sync` finishes them.
    if status.status == "catalysts_pending" {
        1
    } else {
        0
    }
}

/// The margins-meetings preset filled for a not-yet-created Workspace, in a
/// throwaway engine home, with folder readings for missing folders dropped.
#[cfg(feature = "recall")]
fn fill_meetings_preset(
    margins_home: &Path,
    config: &margins_workflows::workspace::WorkspaceConfig,
    folder: &Path,
) -> Result<margins_workflows::workspace_preset::PresetProposal, margins_cli::CliError> {
    use margins_cli::CliError;
    use margins_workflows::workspace_preset;
    let scratch = tempfile::tempdir().map_err(|error| {
        CliError::new("engine_unavailable", format!("creating a scratch engine home: {error}"))
    })?;
    let template = scratch
        .path()
        .join(format!("{}.enzyme.in", workspace_preset::MEETINGS_PRESET));
    std::fs::write(&template, workspace_preset::MEETINGS_PRESET_TEXT)
        .map_err(|error| CliError::from_anyhow(error.into()))?;
    let engine = crate::enzyme_cli::Engine::for_scratch_home(margins_home, &scratch.path().join("home"))
        .map_err(|error| CliError::new("engine_unavailable", format!("{error:#}")))?;
    let filled = engine
        .compile_preset(&config.id, &template, folder)
        .map_err(|error| CliError::new("workspace_preset_invalid", error.to_string()))?;
    workspace_preset::propose(config, &filled)
        .map_err(|error| CliError::new("workspace_preset_invalid", format!("{error:#}")))
}

/// When no catalyst generator is usable, offer the one choice inline: hosted
/// (no download) or local (a model download whose size is stated first).
/// Without a terminal, or with `--json`, nothing is asked; init indexes only
/// and its receipt names the command that turns catalysts on. Readiness is
/// read directly afterwards, never inferred from a setup exit code.
#[cfg(feature = "recall")]
fn offer_catalysts(margins_home: &Path, json: bool) {
    if crate::recall::ensure_usable_generator_at(margins_home).is_ok() {
        return;
    }
    let assume = std::env::var_os(INIT_ASSUME_TERMINAL_ENV).is_some_and(|value| value == "1");
    if json || !(assume || (io::stdin().is_terminal() && io::stderr().is_terminal())) {
        return;
    }
    let local_size = local_model_size(margins_home);
    eprintln!();
    eprintln!("Search works now. Catalysts add related notes that share no words with your query:");
    eprintln!("Margins learns the questions your notes keep raising. Choose how it writes them:");
    eprintln!("  1) Hosted — no download; excerpts of your notes are sent to Margins' hosted service");
    match &local_size {
        Some(size) => eprintln!("  2) Local  — runs on this computer; downloads a {size} model once"),
        None => eprintln!("  2) Local  — not available in this build"),
    }
    eprintln!("  3) Not now — `margins setup --only catalyst` turns them on later");
    eprint!("Choice [1/2/3, default 3]: ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    let _ = io::stdin().lock().read_line(&mut answer);
    let outcome = match answer.trim().to_ascii_lowercase().as_str() {
        "1" | "h" | "hosted" => crate::hosted_credentials::provision_hosted_from_broker(margins_home)
            .and_then(|_| crate::hosted_credentials::set_llm_mode(margins_home, "hosted"))
            .map(|()| true),
        "2" | "l" | "local" if local_size.is_some() => install_local_catalyst(margins_home).map(|()| true),
        _ => Ok(false),
    };
    match outcome {
        Ok(false) => {}
        Ok(true) if crate::recall::ensure_usable_generator_at(margins_home).is_ok() => {
            let mode = margins_workflows::catalyst::selected_status(margins_home).mode;
            eprintln!("Catalysts: {} — ready", mode.as_str());
        }
        Ok(true) => eprintln!("Catalysts are still not usable; indexing only for now."),
        Err(error) => eprintln!(
            "Catalysts could not be set up ({}); indexing only for now.",
            margins_user_message(&format!("{error:#}"))
        ),
    }
    eprintln!();
}

/// The local catalyst model's download size from the engine's registry.
#[cfg(feature = "recall-local-model")]
fn local_model_size(margins_home: &Path) -> Option<String> {
    let models = crate::enzyme_cli::Engine::for_inspection(margins_home).ok()?.models().ok()?;
    let entry = models
        .selected
        .as_deref()
        .and_then(|selected| models.models.iter().find(|model| model.name == selected))
        .or_else(|| models.models.iter().find(|model| model.registry))?;
    Some(format!("~{} MB", entry.size_bytes.div_ceil(1_048_576)))
}

#[cfg(all(feature = "recall", not(feature = "recall-local-model")))]
fn local_model_size(_margins_home: &Path) -> Option<String> {
    None
}

#[cfg(feature = "recall-local-model")]
fn install_local_catalyst(margins_home: &Path) -> anyhow::Result<()> {
    crate::catalyst_model_setup::ensure_installed()?;
    margins_workflows::machine_config::set_generation(margins_home, "local")
}

#[cfg(all(feature = "recall", not(feature = "recall-local-model")))]
fn install_local_catalyst(_margins_home: &Path) -> anyhow::Result<()> {
    anyhow::bail!("this build cannot install a local catalyst model")
}

/// `margins status [--explain] [--all] [--json]`: one engine status read
/// (about 0.6 s on a 7,000-note Workspace); `--explain` adds one read-only
/// `spec plan`. Never refreshes or writes.
#[cfg(feature = "recall")]
fn run_status_journey(selector: Option<&str>, explain: bool, all: bool, json: bool) -> i32 {
    use margins_cli::commands::status;
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let (workspace, selected_by) = match status::select(selector, &cwd) {
        Ok(selected) => selected,
        Err(error) => return report_for(json, error),
    };
    let margins_home = match margins_workflows::workspace::margins_home() {
        Ok(home) => home,
        Err(error) => return report_for(json, margins_cli::CliError::from_anyhow(error)),
    };
    let redacted = crate::hosted_credentials::redacted_status(&margins_home);
    let mut catalyst = status::CatalystView {
        mode: redacted["mode"].as_str().unwrap_or("none").to_string(),
        usable: redacted["usable"].as_bool().unwrap_or(false),
        reason: redacted["reason"].as_str().unwrap_or("unknown").to_string(),
        ..Default::default()
    };
    let engine = match engine_view(&margins_home, &workspace, explain) {
        Ok(view) => view,
        Err(error) => {
            return report_for(
                json,
                margins_cli::CliError::new(
                    "status_unavailable",
                    margins_user_message(&format!("reading the Workspace index: {error:#}")),
                ),
            )
        }
    };
    let margins = margins_cli::commands::workspace_text::margins_for(
        &workspace.config.id,
        margins_cli::commands::workspace_text::is_machine_default(&workspace.config.id),
    );
    catalyst.next_step = if !catalyst.usable {
        Some("Turn catalysts on: `margins setup --only catalyst`".to_string())
    } else if engine.index.state != "indexed" {
        Some(format!("Build the index: `{margins} init`"))
    } else {
        None
    };
    let services = margins_cli::standalone_services();
    let report = match status::build(
        &workspace,
        selected_by,
        catalyst,
        Some(engine),
        all,
        status::connections(&margins_home),
        status::integrations(&workspace),
        status::captures(&services, &workspace),
    ) {
        Ok(report) => report,
        Err(error) => return report_for(json, error),
    };
    match status::write(&report, json, all, &mut io::stdout()) {
        Ok(()) => 0,
        Err(error) => report_for(json, error),
    }
}

/// The engine's view of one Workspace for `status`, read-only.
#[cfg(feature = "recall")]
fn engine_view(
    margins_home: &Path,
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    explain: bool,
) -> anyhow::Result<margins_cli::commands::status::EngineView> {
    use margins_cli::commands::status::{EngineView, EntityView, IndexView, NotesChanged};
    if !workspace.recall_path().is_file() {
        return Ok(EngineView {
            index: IndexView {
                state: "not_built".to_string(),
                ..Default::default()
            },
            ..Default::default()
        });
    }
    let engine = crate::enzyme_cli::Engine::for_inspection(margins_home)?;
    let status = engine.status(&workspace.config.id)?;
    if !status.initialized {
        return Ok(EngineView {
            index: IndexView {
                state: "not_built".to_string(),
                ..Default::default()
            },
            ..Default::default()
        });
    }
    let state = if status.needs_rebuild() { "outdated" } else { "indexed" };
    let snapshot = crate::attention::snapshot(&status, "", "");
    let entities = snapshot
        .entities
        .into_iter()
        .map(|entity| EntityView {
            entity_type: entity.entity_type,
            name: entity.name,
            origin: entity.origin,
            state: entity.state,
            catalysts: entity.catalysts,
            skip_kind: entity.skip_kind,
        })
        .collect();
    let explain = if explain && state == "indexed" {
        Some(explain_view(&engine.spec_plan(
            &workspace.config.id,
            &workspace.state_dir,
            None,
        )?))
    } else {
        None
    };
    Ok(EngineView {
        index: IndexView {
            state: state.to_string(),
            documents: status.documents,
            notes_changed: status.markdown.as_ref().map(|markdown| NotesChanged {
                new: markdown.files_new,
                modified: markdown.files_modified,
                deleted: markdown.files_deleted,
            }),
        },
        entities,
        catalysts: status.catalysts,
        sources: status
            .sources
            .iter()
            .map(|source| {
                (
                    source.name.clone(),
                    (source.documents, source.stale, source.stale_reason.clone(), source.last_refresh_ms),
                )
            })
            .collect(),
        explain,
    })
}

/// `--explain` from `enzyme.spec-plan.v1`: the explain view (`entities`,
/// `skipped`, `counts`) of enzyme 0.12.2, or, from the pinned 0.12.1, the
/// per-reading jobs and skips (TODO(E6): drop that branch with the pin).
#[cfg(feature = "recall")]
fn explain_view(plan: &serde_json::Value) -> margins_cli::commands::status::ExplainView {
    use margins_cli::commands::status::{why, ExplainReading, ExplainSkip, ExplainView};
    let empty = Vec::new();
    let vault = &plan["vaults"][0];
    let readings = vault["readings"].as_array().unwrap_or(&empty);
    let mut groups = readings
        .iter()
        .map(|reading| ExplainReading {
            reading: reading["source"].as_str().unwrap_or("a reading").to_string(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let automatic = groups.len();
    groups.push(ExplainReading {
        reading: "picked automatically".to_string(),
        ..Default::default()
    });
    let group_of = |value: &serde_json::Value| {
        value
            .as_u64()
            .map(|index| index as usize)
            .filter(|index| *index < automatic)
            .unwrap_or(automatic)
    };
    let mut view = ExplainView::default();
    // The explain view sits beside the readings in the planned vault.
    let plan = if vault["entities"].is_array() { vault } else { plan };
    if let Some(entities) = plan["entities"].as_array() {
        for entity in entities {
            let name = entity["name"].as_str().unwrap_or("?");
            let group = &mut groups[group_of(&entity["reading"])];
            if entity["status"] == "planned" {
                group.learns.push(match entity["origin"].as_str() {
                    Some("expanded") => format!("{name} (linked page)"),
                    _ => name.to_string(),
                });
            } else {
                let code = entity["skip"]["code"].as_str().unwrap_or("skipped");
                group.skipped.push(ExplainSkip {
                    code: code.to_string(),
                    what: name.to_string(),
                    why: why(code, entity["skip"]["reason"].as_str()),
                });
            }
        }
        for skip in plan["skipped"].as_array().unwrap_or(&empty) {
            if skip["scope"] == "entity" {
                continue; // repeated from `entities`
            }
            let code = skip["code"].as_str().unwrap_or("skipped");
            let count = skip["count"].as_u64().unwrap_or(1);
            let what = match skip["name"].as_str() {
                Some(name) => name.to_string(),
                None if skip["scope"] == "candidates" => format!(
                    "{count} more {}",
                    if count == 1 { "page" } else { "pages" }
                ),
                None => format!("{count} more"),
            };
            groups[group_of(&skip["reading"])].skipped.push(ExplainSkip {
                code: code.to_string(),
                what,
                why: why(code, skip["reason"].as_str()),
            });
        }
        let counts = &plan["counts"];
        view.planned = counts["planned"].as_u64().unwrap_or(0) as usize;
        view.skipped = counts["skipped"].as_u64().unwrap_or(0) as usize;
        view.candidates_skipped = counts["candidates_skipped"].as_u64().unwrap_or(0) as usize;
        view.truncated = plan["truncated"].is_object();
    } else {
        let jobs = |group: &mut ExplainReading, jobs: &serde_json::Value| {
            for job in jobs.as_array().unwrap_or(&Vec::new()) {
                if let Some(name) = job["entity_name"].as_str() {
                    group.learns.push(name.to_string());
                }
            }
        };
        let skips = |group: &mut ExplainReading, skipped: &serde_json::Value| {
            for skip in skipped.as_array().unwrap_or(&Vec::new()) {
                let code = skip["reason"]["kind"].as_str().unwrap_or("skipped");
                group.skipped.push(ExplainSkip {
                    code: code.to_string(),
                    what: skip["entity_name"].as_str().unwrap_or("?").to_string(),
                    why: why(code, None),
                });
            }
        };
        for (index, reading) in readings.iter().enumerate() {
            jobs(&mut groups[index], &reading["jobs"]);
            skips(&mut groups[index], &reading["skipped"]);
        }
        jobs(&mut groups[automatic], &vault["other_jobs"]);
        skips(&mut groups[automatic], &vault["other_skipped"]);
        view.planned = vault["totals"]["jobs"].as_u64().unwrap_or(0) as usize;
        view.skipped = vault["totals"]["skipped"].as_u64().unwrap_or(0) as usize;
    }
    if groups[automatic].learns.is_empty() && groups[automatic].skipped.is_empty() {
        groups.pop();
    }
    view.readings = groups;
    view
}

/// `margins edit`'s effect preview: what the edited program would have the
/// next catalyst build learn about, with the notes as indexed now (read-only
/// `spec plan` of the desired program). Empty when there is no index yet or
/// the engine cannot say.
#[cfg(feature = "recall")]
fn edit_preview(
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    program: &str,
) -> Vec<String> {
    let run = || -> anyhow::Result<Vec<String>> {
        if !workspace.recall_path().is_file() {
            return Ok(Vec::new());
        }
        let margins_home = margins_workflows::workspace::margins_home()?;
        let engine = crate::enzyme_cli::Engine::for_inspection(&margins_home)?;
        let file = tempfile::Builder::new().suffix(".enzyme").tempfile()?;
        std::fs::write(file.path(), program)?;
        let view = explain_view(&engine.spec_plan(
            &workspace.config.id,
            &workspace.state_dir,
            Some(file.path()),
        )?);
        let mut lines = Vec::new();
        for reading in &view.readings {
            lines.push(format!(
                "{}: {}",
                reading.reading,
                if reading.learns.is_empty() {
                    "nothing yet".to_string()
                } else {
                    reading.learns.join(", ")
                }
            ));
            lines.extend(
                reading
                    .skipped
                    .iter()
                    .map(|skip| format!("  {} — {}", skip.what, skip.why)),
            );
        }
        lines.push(format!(
            "In total: {} learned about, {} skipped",
            view.planned, view.skipped
        ));
        Ok(lines)
    };
    run().unwrap_or_default()
}
