#[derive(Clone, Debug, Eq, PartialEq)]
enum HostedCatalystSetup {
    Hosted { model: String },
    Offline { reason: String },
}

trait SetupMachineProvisioner {
    fn provision_hosted_catalyst(&self, margins_home: &Path) -> Result<HostedCatalystSetup>;
    fn provision_speech(&self) -> Result<Option<std::path::PathBuf>>;
    fn provision_local_catalyst(&self) -> Result<Option<std::path::PathBuf>>;
}

struct NativeSetupMachineProvisioner;

static MARGINS_SKILL: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skills/margins");
static WATERMARK_SKILL: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skills/watermark");

const EMBEDDED_SKILLS: [(&str, &Dir<'static>); 2] =
    [("margins", &MARGINS_SKILL), ("watermark", &WATERMARK_SKILL)];

impl SetupMachineProvisioner for NativeSetupMachineProvisioner {
    fn provision_hosted_catalyst(&self, margins_home: &Path) -> Result<HostedCatalystSetup> {
        #[cfg(feature = "recall")]
        {
            return match crate::hosted_credentials::provision_hosted_or_local(margins_home)? {
                crate::hosted_credentials::HostedSetupOutcome::Hosted(report) => {
                    Ok(HostedCatalystSetup::Hosted {
                        model: report.model,
                    })
                }
                crate::hosted_credentials::HostedSetupOutcome::LocalFallback { reason } => {
                    Ok(HostedCatalystSetup::Offline { reason })
                }
            };
        }
        #[cfg(not(feature = "recall"))]
        {
            let _ = margins_home;
            bail!("hosted catalyst provisioning is unavailable in this build")
        }
    }

    fn provision_speech(&self) -> Result<Option<std::path::PathBuf>> {
        ensure_speech_model(true)?;
        Ok(margins_media::model_registry::resolve_coreml_dir())
    }

    fn provision_local_catalyst(&self) -> Result<Option<std::path::PathBuf>> {
        #[cfg(feature = "recall-local-model")]
        {
            return crate::catalyst_model_setup::ensure_installed().map(Some);
        }
        #[cfg(not(feature = "recall-local-model"))]
        {
            Ok(None)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SetupSelection {
    catalyst: bool,
    skills: bool,
    speech: bool,
    local_model: SetupLocalModelPolicyArg,
}

impl SetupSelection {
    fn from_args(
        only: &[SetupStepArg],
        skip: Option<SetupSkipArg>,
        local_model: SetupLocalModelPolicyArg,
    ) -> Self {
        let all = only.is_empty();
        Self {
            catalyst: all || only.contains(&SetupStepArg::Catalyst),
            skills: all || only.contains(&SetupStepArg::Skills),
            speech: (all || only.contains(&SetupStepArg::Speech))
                && skip != Some(SetupSkipArg::Speech),
            local_model,
        }
    }
}

/// Machine-level setup is deliberately directory-agnostic. Each selected step
/// runs even when an earlier step failed; the final handoff names only an
/// explicitly selected Workspace and never derives one from the process cwd.
fn run_setup(
    workspace_selector: Option<&str>,
    only: &[SetupStepArg],
    skip: Option<SetupSkipArg>,
    local_model: SetupLocalModelPolicyArg,
) -> i32 {
    let selection = SetupSelection::from_args(only, skip, local_model);
    let skill_home = home_dir();
    let margins_home = margins_workflows::workspace::margins_home();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    let env_workspace = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let workspace = workspace_selector.or(env_workspace.as_deref());
    match run_setup_with(
        &NativeSetupMachineProvisioner,
        selection,
        margins_home.as_deref().ok(),
        margins_home
            .as_ref()
            .err()
            .map(ToString::to_string)
            .as_deref(),
        skill_home.as_deref(),
        workspace,
        &mut stdout,
        &mut stderr,
    ) {
        Ok(failed) => i32::from(failed),
        Err(error) => report_error(&format!("writing setup report: {error}")),
    }
}

fn run_setup_with(
    provisioner: &dyn SetupMachineProvisioner,
    selection: SetupSelection,
    margins_home: Option<&Path>,
    margins_home_error: Option<&str>,
    skill_home: Option<&Path>,
    workspace: Option<&str>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<bool> {
    let mut hosted_ready = false;
    let mut local_model_failed = false;
    let mut selected_step_failed = false;

    if selection.catalyst {
        match margins_home {
            Some(home) => match provisioner.provision_hosted_catalyst(home) {
                Ok(HostedCatalystSetup::Hosted { model }) => {
                    hosted_ready = true;
                    setup_status(
                        stderr,
                        "hosted catalyst",
                        true,
                        &format!("ready with {model}"),
                    )?;
                }
                Ok(HostedCatalystSetup::Offline { reason }) => {
                    setup_status(stderr, "hosted catalyst", false, &reason)?;
                }
                Err(error) => {
                    setup_status(stderr, "hosted catalyst", false, &format!("{error:#}"))?;
                }
            },
            None => {
                setup_status(
                    stderr,
                    "hosted catalyst",
                    false,
                    margins_home_error.unwrap_or("could not resolve MARGINS_HOME"),
                )?;
            }
        }
    }

    if selection.skills {
        match skill_home {
            Some(home) => match install_embedded_skills(home, stderr) {
                Ok(()) => setup_status(
                    stderr,
                    "skills",
                    true,
                    "installed embedded margins and watermark skills",
                )?,
                Err(error) => {
                    selected_step_failed = true;
                    setup_status(stderr, "skills", false, &format!("{error:#}"))?;
                }
            },
            None => {
                selected_step_failed = true;
                setup_status(
                    stderr,
                    "skills",
                    false,
                    "could not resolve HOME for agent skill installation",
                )?;
            }
        }
    }

    if selection.speech {
        match provisioner.provision_speech() {
            Ok(Some(model_dir)) => setup_status(
                stderr,
                "speech",
                true,
                &format!(
                    "model cache {} ({})",
                    model_dir.display(),
                    human_bytes(dir_size(&model_dir))
                ),
            )?,
            Ok(None) => setup_status(
                stderr,
                "speech",
                true,
                "no local speech model is required by this build",
            )?,
            Err(error) => {
                selected_step_failed = true;
                setup_status(stderr, "speech", false, &format!("{error:#}"))?;
            }
        }
    }

    if selection.catalyst {
        let install_local =
            !hosted_ready || selection.local_model == SetupLocalModelPolicyArg::Always;
        if install_local {
            match provisioner.provision_local_catalyst() {
                Ok(Some(path)) => {
                    // An installed local model is the generator: chosen by
                    // `--local-model always`, or the fallback without hosted.
                    let selected = margins_home
                        .context("could not resolve MARGINS_HOME")
                        .and_then(|home| {
                            margins_workflows::machine_config::set_generation(home, "local")
                        });
                    match selected {
                        Ok(()) => setup_status(
                            stderr,
                            "local catalyst",
                            true,
                            &format!(
                                "installed at {} ({})",
                                path.display(),
                                human_bytes(
                                    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
                                )
                            ),
                        )?,
                        Err(error) => {
                            local_model_failed = true;
                            setup_status(stderr, "local catalyst", false, &format!("{error:#}"))?;
                        }
                    }
                }
                Ok(None) => setup_status(
                    stderr,
                    "local catalyst",
                    true,
                    "selected without a bundled local-model installer",
                )?,
                Err(error) => {
                    local_model_failed = true;
                    setup_status(stderr, "local catalyst", false, &format!("{error:#}"))?;
                }
            }
        } else {
            setup_status(
                stderr,
                "local catalyst",
                true,
                "not installed under fallback policy because hosted catalyst is selected",
            )?;
        }
    }

    // The catalyst line is always reported, but it only decides the exit code
    // when the catalyst step was selected. `--only skills|speech` succeeds or
    // fails on its own steps; an unconfigured catalyst is just a note there.
    let catalyst_note = if selection.catalyst {
        ""
    } else {
        " (not part of this setup; run margins setup --only catalyst)"
    };
    let usable_generator = if let Some(home) = margins_home {
        let catalyst = margins_workflows::catalyst::selected_status(home);
        writeln!(
            stderr,
            "catalyst mode: {} — {}{catalyst_note}",
            catalyst.mode.as_str(),
            catalyst.reason
        )?;
        match catalyst.mode {
            margins_workflows::catalyst::CatalystMode::Hosted => true,
            margins_workflows::catalyst::CatalystMode::Local => !local_model_failed,
            margins_workflows::catalyst::CatalystMode::None => false,
        }
    } else {
        writeln!(
            stderr,
            "catalyst mode: none — config_unreadable{catalyst_note}"
        )?;
        false
    };

    margins_cli::commands::guide::setup_handoff(workspace, stdout)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    // With the catalyst selected (including full setup), a usable generator is
    // the success criterion, as before; other step failures are reported only.
    if selection.catalyst {
        Ok(!usable_generator)
    } else {
        Ok(selected_step_failed)
    }
}

fn setup_status(report: &mut dyn Write, step: &str, ok: bool, reason: &str) -> Result<()> {
    let reason = margins_user_message(reason);
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    writeln!(
        report,
        "setup {step}: {} — {reason}",
        if ok { "ok" } else { "failed" }
    )?;
    Ok(())
}

fn install_embedded_skills(home: &Path, report: &mut dyn Write) -> Result<()> {
    let canonical_root = home.join(".margins/skills");
    for (name, embedded) in EMBEDDED_SKILLS {
        let destination = canonical_root.join(name);
        remove_existing_canonical_skill(&destination)?;
        materialize_embedded_dir(embedded, &destination)?;
    }
    let canonical_root = std::fs::canonicalize(&canonical_root).with_context(|| {
        format!(
            "failed to resolve canonical skill directory {}",
            canonical_root.display()
        )
    })?;

    for (agent, agent_home_name) in AGENT_SKILL_DIRS {
        let agent_home = home.join(agent_home_name);
        if !agent_home.is_dir() {
            writeln!(
                report,
                "{} skills: skipped (not installed)",
                agent.display()
            )?;
            continue;
        }
        install_agent_skill_links(agent.display(), &agent_home, &canonical_root, report)?;
    }
    Ok(())
}

fn remove_existing_canonical_skill(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to replace canonical skill {}", path.display()))?,
        Ok(_) => std::fs::remove_file(path)
            .with_context(|| format!("failed to replace canonical skill {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect canonical skill {}", path.display()));
        }
    }
    Ok(())
}

fn materialize_embedded_dir(embedded: &Dir<'_>, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).with_context(|| {
        format!(
            "failed to create embedded skill directory {}",
            destination.display()
        )
    })?;

    for file in embedded.files() {
        let name = file.path().file_name().with_context(|| {
            format!("embedded skill file has no name: {}", file.path().display())
        })?;
        let destination = destination.join(name);
        std::fs::write(&destination, file.contents())
            .with_context(|| format!("failed to write embedded skill {}", destination.display()))?;
    }
    for directory in embedded.dirs() {
        let name = directory.path().file_name().with_context(|| {
            format!(
                "embedded skill directory has no name: {}",
                directory.path().display()
            )
        })?;
        materialize_embedded_dir(directory, &destination.join(name))?;
    }
    Ok(())
}

#[cfg(unix)]
fn install_agent_skill_links(
    agent: &str,
    agent_home: &Path,
    canonical_root: &Path,
    report: &mut dyn Write,
) -> Result<()> {
    let skills_dir = agent_home.join("skills");
    std::fs::create_dir_all(&skills_dir).with_context(|| {
        format!(
            "failed to create {agent} skill directory {}",
            skills_dir.display()
        )
    })?;

    let mut linked = Vec::new();
    let mut preserved = Vec::new();
    for (skill, _) in EMBEDDED_SKILLS {
        let target = canonical_root.join(skill);
        let link = skills_dir.join(skill);
        match std::fs::symlink_metadata(&link) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if std::fs::canonicalize(&link).ok() != Some(target.clone()) {
                    std::fs::remove_file(&link).with_context(|| {
                        format!(
                            "failed to remove stale {agent} skill link {}",
                            link.display()
                        )
                    })?;
                    std::os::unix::fs::symlink(&target, &link).with_context(|| {
                        format!("failed to link {agent} skill {}", link.display())
                    })?;
                }
                linked.push(skill);
            }
            Ok(_) => preserved.push(skill),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::os::unix::fs::symlink(&target, &link)
                    .with_context(|| format!("failed to link {agent} skill {}", link.display()))?;
                linked.push(skill);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect {agent} skill path {}", link.display())
                });
            }
        }
    }

    write_agent_skill_report(agent, &linked, &preserved, report)
}

#[cfg(not(unix))]
fn install_agent_skill_links(
    agent: &str,
    _agent_home: &Path,
    _canonical_root: &Path,
    report: &mut dyn Write,
) -> Result<()> {
    writeln!(
        report,
        "{agent} skills: skipped (symlinks unsupported on this platform)"
    )?;
    Ok(())
}

#[cfg(unix)]
fn write_agent_skill_report(
    agent: &str,
    linked: &[&str],
    preserved: &[&str],
    report: &mut dyn Write,
) -> Result<()> {
    match (linked.is_empty(), preserved.is_empty()) {
        (false, true) => writeln!(report, "{agent} skills: linked {}", linked.join(", "))?,
        (true, false) => writeln!(
            report,
            "{agent} skills: skipped (user dir present: {})",
            preserved.join(", ")
        )?,
        (false, false) => writeln!(
            report,
            "{agent} skills: linked {}; skipped {} (user dir present)",
            linked.join(", "),
            preserved.join(", ")
        )?,
        (true, true) => unreachable!("Margins always embeds at least one skill"),
    }
    Ok(())
}

pub(crate) fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

/// Recursively sum file sizes under `path`; best-effort, ignores unreadable entries.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => total += dir_size(&entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// ── Speech model setup ────────────────────────────────────────────────────────

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn ensure_speech_model(explicit_setup: bool) -> Result<()> {
    if margins_media::model_registry::resolve_coreml_dir().is_some() {
        if explicit_setup {
            let speaker_setup = start_speaker_recognition()?;
            finish_speaker_recognition(speaker_setup, 0)?;
            print_local_speech_ready()?;
        }
        return Ok(());
    }

    let mut stderr = io::stderr().lock();
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "local transcription is not installed; run `margins setup` in a terminal before `margins new`"
        );
    }
    write!(
        stderr,
        "Local speech needs a one-time download. Download now? [Y/n] "
    )?;
    stderr.flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let accepted = matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    );
    if !accepted {
        if explicit_setup {
            anyhow::bail!("setup canceled");
        }
        anyhow::bail!(
            "local transcription is required before `margins new`; run `margins setup` when ready"
        );
    }

    let speaker_setup = start_speaker_recognition()?;
    let terminal = stderr.is_terminal();
    let mut last_percent = u8::MAX;
    let mut last_render = std::time::Instant::now();
    let mut transfer_started = None;
    let mut starting_bytes = None;
    let mut transcription_total = 0;
    margins_media::model_registry::coreml::download_model(|downloaded, total| {
        transcription_total = total;
        let speaker_downloaded = speaker_setup.downloaded_bytes();
        let combined_downloaded = downloaded.saturating_add(speaker_downloaded);
        let combined_total = total.saturating_add(speaker_setup.total_bytes());
        let initial = *starting_bytes.get_or_insert(combined_downloaded);
        let started = *transfer_started.get_or_insert_with(std::time::Instant::now);
        let percent = if combined_total == 0 {
            0
        } else {
            ((combined_downloaded.saturating_mul(100) / combined_total).min(100)) as u8
        };
        if percent == last_percent && last_render.elapsed() < std::time::Duration::from_millis(250)
        {
            return;
        }
        last_percent = percent;
        last_render = std::time::Instant::now();
        let transferred = combined_downloaded.saturating_sub(initial);
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let bytes_per_second = transferred as f64 / elapsed;
        let remaining = combined_total.saturating_sub(combined_downloaded);
        let eta = if bytes_per_second >= 1.0 {
            Some(remaining as f64 / bytes_per_second)
        } else {
            None
        };
        let action = if initial > 0 {
            "Resuming"
        } else {
            "Downloading"
        };
        if terminal {
            let filled = usize::from(percent) / 4;
            let bar = format!("{}{}", "=".repeat(filled), " ".repeat(25 - filled));
            let downloaded_mb = combined_downloaded as f64 / 1_048_576.0;
            let total_mb = combined_total as f64 / 1_048_576.0;
            let speed_mb = bytes_per_second / 1_048_576.0;
            let eta = eta
                .map(|seconds| format!("  {:>3}s", seconds.ceil() as u64))
                .unwrap_or_default();
            let _ = write!(
                stderr,
                "\r{action} local speech [{bar}] {percent:>3}%  {downloaded_mb:.0}/{total_mb:.0} MB  {speed_mb:.1} MB/s{eta}   "
            );
            let _ = stderr.flush();
        } else if percent % 10 == 0 {
            let _ = writeln!(stderr, "Downloading: {percent}%");
        }
    })?;
    drop(stderr);
    finish_speaker_recognition(speaker_setup, transcription_total)?;
    print_local_speech_ready()?;
    Ok(())
}

#[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
fn ensure_speech_model(_explicit_setup: bool) -> Result<()> {
    // Non-macOS/non-CoreML builds skip model setup silently and let capture proceed.
    Ok(())
}

#[cfg(feature = "polyvoice-diarization")]
struct SpeakerSetup {
    preparation: std::thread::JoinHandle<std::result::Result<(), String>>,
    files: Vec<(std::path::PathBuf, u64)>,
}

#[cfg(feature = "polyvoice-diarization")]
impl SpeakerSetup {
    fn total_bytes(&self) -> u64 {
        self.files.iter().map(|(_, size)| size).sum()
    }

    fn downloaded_bytes(&self) -> u64 {
        self.files
            .iter()
            .map(|(path, size)| {
                let complete = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                let partial_name = format!(
                    ".{}.partial",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                let partial = path
                    .parent()
                    .and_then(|parent| std::fs::metadata(parent.join(partial_name)).ok())
                    .map(|m| m.len())
                    .unwrap_or(0);
                complete.max(partial).min(*size)
            })
            .sum()
    }
}

#[cfg(feature = "polyvoice-diarization")]
fn start_speaker_recognition() -> Result<SpeakerSetup> {
    let files = margins_media::providers::polyvoice::balanced_model_downloads()
        .context("could not inspect speaker recognition models")?;
    let preparation = std::thread::spawn(|| {
        margins_media::providers::polyvoice::PolyvoiceDiarization::from_default_registry()
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    Ok(SpeakerSetup { preparation, files })
}

#[cfg(not(feature = "polyvoice-diarization"))]
struct SpeakerSetup {
    preparation: std::thread::JoinHandle<std::result::Result<(), String>>,
}

#[cfg(not(feature = "polyvoice-diarization"))]
impl SpeakerSetup {
    fn total_bytes(&self) -> u64 {
        0
    }
    fn downloaded_bytes(&self) -> u64 {
        0
    }
}

#[cfg(not(feature = "polyvoice-diarization"))]
fn start_speaker_recognition() -> Result<SpeakerSetup> {
    anyhow::bail!("this Margins build does not include speaker recognition")
}

fn finish_speaker_recognition(
    setup: SpeakerSetup,
    completed_transcription_bytes: u64,
) -> Result<()> {
    let mut stderr = io::stderr().lock();
    let terminal = stderr.is_terminal();
    let speaker_total = setup.total_bytes();
    let combined_total = completed_transcription_bytes.saturating_add(speaker_total);
    while !setup.preparation.is_finished() && setup.downloaded_bytes() < speaker_total {
        if terminal && combined_total > 0 {
            let downloaded = completed_transcription_bytes.saturating_add(setup.downloaded_bytes());
            let percent = ((downloaded.saturating_mul(100) / combined_total).min(100)) as u8;
            let filled = usize::from(percent) / 4;
            let bar = format!("{}{}", "=".repeat(filled), " ".repeat(25 - filled));
            write!(stderr, "\rInstalling local speech [{bar}] {percent:>3}%   ")?;
            stderr.flush()?;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(stderr);
    finish_animated_task("Validating local speech", setup.preparation)
}

fn finish_animated_task(
    active_label: &str,
    preparation: std::thread::JoinHandle<std::result::Result<(), String>>,
) -> Result<()> {
    let mut stderr = io::stderr().lock();
    let terminal = stderr.is_terminal();
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut frame = 0;
    let started = std::time::Instant::now();
    while !preparation.is_finished() {
        if terminal {
            write!(
                stderr,
                "\r  {}  {active_label}  ·  {:.1}s   ",
                frames[frame % frames.len()],
                started.elapsed().as_secs_f64(),
            )?;
            stderr.flush()?;
            frame += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let result = preparation
        .join()
        .map_err(|_| anyhow::anyhow!("{active_label} stopped unexpectedly"))?;
    result.map_err(|error| anyhow::anyhow!("{active_label} failed: {error}"))?;
    Ok(())
}

fn print_local_speech_ready() -> Result<()> {
    let mut stderr = io::stderr().lock();
    if stderr.is_terminal() {
        writeln!(stderr, "\r\x1b[2K  ✓  Local speech ready")?;
    } else {
        writeln!(stderr, "Local speech ready.")?;
    }
    Ok(())
}
