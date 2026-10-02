#[cfg(feature = "audio-capture")]
fn post_new_session_hint(session_name: &str) -> String {
    let home = home_dir();
    post_new_session_hint_for_home(home.as_deref(), session_name)
}

fn post_new_session_hint_for_home(home: Option<&Path>, _session_name: &str) -> String {
    if distillation_skill_installed_for_home(home) {
        "To distill your latest session, run `margins note`.".to_string()
    } else {
        "Run `margins setup` once to finish setup, then `margins note`.".to_string()
    }
}

fn distillation_skill_installed_for_home(home: Option<&Path>) -> bool {
    home.map(|home| home.join(".margins/skills/margins/SKILL.md").is_file())
        .unwrap_or(true)
}

#[cfg(any(test, feature = "audio-capture"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostCaptureAction {
    Distill,
    SavedOnly,
    NotOffered,
}

#[cfg(any(test, feature = "audio-capture"))]
fn ask_post_capture_action(
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<PostCaptureAction> {
    loop {
        write!(output, "Turn this session into a note? [Y/n] ")?;
        output.flush()?;

        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            return Ok(PostCaptureAction::SavedOnly);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(PostCaptureAction::Distill),
            "n" | "no" => return Ok(PostCaptureAction::SavedOnly),
            _ => writeln!(output, "Please answer y or n.")?,
        }
    }
}

#[cfg(feature = "audio-capture")]
fn choose_post_capture_action() -> Result<PostCaptureAction> {
    let stdin = io::stdin();
    let stderr = io::stderr();
    let home = home_dir();
    if !stdin.is_terminal()
        || !stderr.is_terminal()
        || !distillation_skill_installed_for_home(home.as_deref())
    {
        return Ok(PostCaptureAction::NotOffered);
    }
    ask_post_capture_action(&mut stdin.lock(), &mut stderr.lock())
}

impl InteractiveSession for NativeInteractiveSession {
    fn create(&self, work_dir: &Path, title: Option<&str>) -> Result<()> {
        #[cfg(not(feature = "audio-capture"))]
        {
            let _ = (work_dir, title);
            bail!("audio capture is unavailable: rebuild Margins with the `audio-capture` feature")
        }
        #[cfg(feature = "audio-capture")]
        {
            ensure_capture_permissions(&NativeCapturePermissionSource)?;
            create_native_session(work_dir, title)
        }
    }

    fn attach(&self, work_dir: &Path, selected: Option<&str>) -> Result<()> {
        #[cfg(not(feature = "audio-capture"))]
        {
            let _ = (work_dir, selected);
            bail!("audio capture is unavailable: rebuild Margins with the `audio-capture` feature")
        }
        #[cfg(feature = "audio-capture")]
        {
            ensure_capture_permissions(&NativeCapturePermissionSource)?;
            attach_native_session(work_dir, selected)
        }
    }
}

#[cfg(feature = "audio-capture")]
fn create_native_session(work_dir: &Path, title: Option<&str>) -> Result<()> {
    ensure_speech_model(false)?;
    let margins_dir = work_dir.join(".margins");
    let started_at = Local::now();
    let name = margins_cli::commands::sessions::unique_session_name(
        &margins_cli::standalone_services(),
        &margins_dir,
        &started_at.format("%Y-%m-%d-%H-%M-%S").to_string(),
    )?;
    let memo_path = work_dir.join(".margins").join(format!("{name}.md"));
    let audio_path = work_dir.join(".margins").join(format!("{name}_seg0.wav"));
    let initial_offset_ms = 0i64;
    let live_artifact_ordinal: i64 = 0;
    let checkpoint_uri = format!(".margins/{name}_seg{live_artifact_ordinal}.live-transcript.json");
    let checkpoint_path = work_dir.join(&checkpoint_uri);
    // Begin model loading as soon as the stable session identity exists. It
    // overlaps with durable session bookkeeping, device lookup, and TUI setup.
    let live_status = Arc::new(AtomicU8::new(crate::app::LIVE_TRANSCRIPTION_WARMING));
    let live = start_live_transcript_worker(
        checkpoint_path,
        initial_offset_ms as u64,
        live_status.clone(),
    );

    // Open both native lanes before reserving any session state. Permission or
    // device-start failures therefore leave no memo, segment, or current
    // pointer behind.
    let initial_live_sink = live
        .as_ref()
        .map(|worker| worker.sink_for_offset(initial_offset_ms as u64));
    let initial_stop = Arc::new(AtomicBool::new(false));
    let initial_recorder = crate::recorder::RecorderHandle::start_with_live_audio(
        initial_stop.clone(),
        None,
        initial_live_sink,
    )?;

    std::fs::create_dir_all(&margins_dir).context("failed to create .margins directory")?;
    // Silent bookkeeping so desktop and `recent --all` can enumerate this folder.
    margins_workflows::project::register_vault_silently(work_dir);
    let mut meeting = capture_local_runtime::LocalMeetingProducer::reserve(
        &margins_dir,
        &name,
        title,
        started_at,
    )?;
    meeting.open(0)?;
    margins_cli::commands::sessions::write_current_session(
        &margins_cli::standalone_services(),
        &margins_dir,
        &name,
    )?;

    let mic_name = crate::recorder::default_input_device_name().unwrap_or_else(|| "Unknown".into());
    let mut app = crate::app::App::new(
        memo_path.to_string_lossy().into_owned(),
        started_at,
        mic_name,
    );
    app.bind_workspace_authority(margins_dir.clone(), name.clone());
    let observed =
        margins_store::SqliteWorkspaceAuthorityStorage::open(&margins_dir)?.memo(&name)?;
    app.observe_memo(observed.revision, observed.lines);
    app.live_transcription_status = live_status;

    let action = run_segment(
        &mut app,
        &margins_dir,
        &name,
        0,
        audio_path,
        live,
        live_artifact_ordinal,
        started_at,
        initial_offset_ms,
        initial_recorder,
        initial_stop,
        &mut meeting,
    )?;
    match action {
        PostCaptureAction::Distill => crate::note::run(false),
        PostCaptureAction::SavedOnly => {
            eprintln!("Session saved.");
            Ok(())
        }
        PostCaptureAction::NotOffered => {
            // One-line pointer at the moment of truth. A first capture nudges
            // setup only until the bundled distillation skill is installed.
            eprintln!("{}", post_new_session_hint(&name));
            Ok(())
        }
    }
}

#[cfg(feature = "audio-capture")]
fn attach_native_session(work_dir: &Path, selected: Option<&str>) -> Result<()> {
    ensure_speech_model(false)?;
    let margins_dir = work_dir.join(".margins");
    let name = match selected {
        Some(name) => name.to_string(),
        None => margins_cli::commands::sessions::read_current_session(
            &margins_cli::standalone_services(),
            &margins_dir,
        )?,
    };
    if name.is_empty() || !margins_store::canonical::session_exists(&margins_dir, &name)? {
        bail!("Session '{name}' not found. Run `margins ls` to choose one.");
    }
    let meta = margins_store::canonical::get_session_meta(&margins_dir, &name)?;
    let started_at = margins_store::canonical::get_session_start_time(&margins_dir, &name)?;
    let memo_path = resolve_artifact(work_dir, &meta.notes_path);
    let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&margins_dir)?;
    let observed = authority.memo(&name)?;
    let parsed = margins_core::TimedMemoDocument::from_committed(observed.lines.clone());
    let mut ordinal = margins_store::canonical::next_segment_index(&margins_dir, &name)?;
    let offset_ms = (Local::now() - started_at).num_milliseconds().max(0);
    let mut meeting = capture_local_runtime::LocalMeetingProducer::recover(
        &margins_dir,
        &name,
        offset_ms as u64,
        meta.title.as_deref(),
        started_at,
    )?;
    let pending_wav = margins_dir.join(format!("{name}_seg{ordinal}.wav"));
    if pending_wav.exists() {
        let prior_offset = meeting.existing_segment_start(ordinal)?.unwrap_or_else(|| {
            let frames = hound::WavReader::open(&pending_wav)
                .map(|reader| reader.duration() as u64)
                .unwrap_or(0);
            (offset_ms as u64).saturating_sub(frames * 1000 / 16_000)
        });
        meeting.open(ordinal)?;
        meeting.ingest_wav_and_close(
            ordinal,
            &pending_wav,
            prior_offset,
            margins_meeting_protocol::SegmentCloseReasonV1::Error,
        )?;
        ordinal = margins_store::canonical::next_segment_index(&margins_dir, &name)?;
    }
    let audio_path = margins_dir.join(format!("{name}_seg{ordinal}.wav"));
    let live_artifact_ordinal = ordinal;
    let initial_offset_ms = offset_ms;
    let checkpoint_uri = format!(".margins/{name}_seg{live_artifact_ordinal}.live-transcript.json");
    let checkpoint_path = work_dir.join(&checkpoint_uri);
    let live_status = Arc::new(AtomicU8::new(crate::app::LIVE_TRANSCRIPTION_WARMING));
    let live = start_live_transcript_worker(
        checkpoint_path,
        initial_offset_ms as u64,
        live_status.clone(),
    );
    let initial_live_sink = live
        .as_ref()
        .map(|worker| worker.sink_for_offset(initial_offset_ms as u64));
    let initial_stop = Arc::new(AtomicBool::new(false));
    let initial_recorder = crate::recorder::RecorderHandle::start_with_live_audio(
        initial_stop.clone(),
        None,
        initial_live_sink,
    )?;
    meeting.open(ordinal)?;
    margins_cli::commands::sessions::write_current_session(
        &margins_cli::standalone_services(),
        &margins_dir,
        &name,
    )?;
    let mic_name = crate::recorder::default_input_device_name().unwrap_or_else(|| "Unknown".into());
    let mut app = crate::app::App::from_memo(
        parsed,
        memo_path.to_string_lossy().into_owned(),
        started_at,
        mic_name,
    );
    app.bind_workspace_authority(margins_dir.clone(), name.clone());
    app.observe_memo(observed.revision, observed.lines);
    app.live_transcription_status = live_status;

    let action = run_segment(
        &mut app,
        &margins_dir,
        &name,
        ordinal,
        audio_path,
        live,
        live_artifact_ordinal,
        started_at,
        initial_offset_ms,
        initial_recorder,
        initial_stop,
        &mut meeting,
    )?;
    match action {
        PostCaptureAction::Distill => crate::note::run(false),
        PostCaptureAction::SavedOnly => {
            eprintln!("Session saved.");
            Ok(())
        }
        PostCaptureAction::NotOffered => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "audio-capture")]
fn run_segment(
    app: &mut crate::app::App,
    margins_dir: &Path,
    session_name: &str,
    initial_ordinal: i64,
    initial_audio_path: PathBuf,
    live: Option<LiveTranscriptWorker>,
    live_artifact_ordinal: i64,
    started_at: chrono::DateTime<Local>,
    initial_offset_ms: i64,
    initial_recorder: crate::recorder::RecorderHandle,
    initial_stop: Arc<AtomicBool>,
    meeting: &mut capture_local_runtime::LocalMeetingProducer,
) -> Result<PostCaptureAction> {
    let mut ordinal = initial_ordinal;
    let mut audio_path = initial_audio_path;
    let mut selected_device: Option<crate::recorder::InputDevice> = None;
    let mut live_timeline_duration_ms: u64 = 0;
    let mut initial_recorder = Some(initial_recorder);
    let mut initial_stop = Some(initial_stop);

    // Memo durability must not depend on the optional transcription path.
    let mut tui_error: Option<anyhow::Error> = None;

    loop {
        let segment_offset_ms = (Local::now() - started_at).num_milliseconds().max(0);
        let stop = initial_stop
            .take()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let recorder = if let Some(recorder) = initial_recorder.take() {
            recorder
        } else {
            let live_sink = live
                .as_ref()
                .map(|worker| worker.sink_for_offset(segment_offset_ms as u64));
            crate::recorder::RecorderHandle::start_with_live_audio(
                stop.clone(),
                selected_device.as_ref(),
                live_sink,
            )?
        };

        app.mic_level = recorder.mic_peak();
        app.spk_level = recorder.spk_peak();
        app.mic_drops = recorder.mic_drops();
        app.spk_drops = recorder.spk_drops();
        app.spk_silence = recorder.spk_silence();
        app.spk_frames = recorder.spk_frames();
        app.spk_rate = recorder.spk_rate();

        let tui_result = crate::tui::run_tui(app, stop.clone())
            .map_err(|error| anyhow::anyhow!(error.to_string()));
        stop.store(true, Ordering::SeqCst);
        let duration = recorder.stop_and_write(&audio_path.to_string_lossy())?;

        let reason = if matches!(&tui_result, Ok(crate::tui::TuiAction::Pause)) {
            margins_meeting_protocol::SegmentCloseReasonV1::Pause
        } else if matches!(&tui_result, Ok(crate::tui::TuiAction::SwitchDevice(_))) {
            margins_meeting_protocol::SegmentCloseReasonV1::Rollover
        } else if tui_result.is_err() {
            margins_meeting_protocol::SegmentCloseReasonV1::Error
        } else {
            margins_meeting_protocol::SegmentCloseReasonV1::Stop
        };
        meeting.ingest_wav_and_close(ordinal, &audio_path, segment_offset_ms as u64, reason)?;
        live_timeline_duration_ms = live_timeline_duration_ms.max(
            (segment_offset_ms as u64)
                .saturating_sub(initial_offset_ms as u64)
                .saturating_add((duration * 1000.0).round().max(0.0) as u64),
        );

        match tui_result {
            Err(error) => {
                tui_error = Some(error);
                break;
            }
            Ok(crate::tui::TuiAction::Quit) => break,
            Ok(crate::tui::TuiAction::Pause) => {
                app.set_capture_paused(true);
                app.mic_level = Arc::new(AtomicU32::new(0));
                app.spk_level = Arc::new(AtomicU32::new(0));
                let resume = loop {
                    let paused_stop = Arc::new(AtomicBool::new(false));
                    match crate::tui::run_tui(app, paused_stop)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?
                    {
                        crate::tui::TuiAction::Resume => break true,
                        crate::tui::TuiAction::Quit => break false,
                        crate::tui::TuiAction::SwitchDevice(index) => {
                            let mut devices = crate::recorder::list_input_devices();
                            if index >= devices.len() {
                                return Err(anyhow::anyhow!(
                                    "selected audio input is no longer available"
                                ));
                            }
                            let (device_name, device) = devices.swap_remove(index);
                            app.current_mic_name = device_name;
                            selected_device = Some(device);
                        }
                        crate::tui::TuiAction::Pause => {}
                    }
                };
                if !resume {
                    break;
                }
                app.set_capture_paused(false);
                ordinal = margins_store::canonical::next_segment_index(margins_dir, session_name)?;
                audio_path = margins_dir.join(format!("{session_name}_seg{ordinal}.wav"));
                meeting.open(ordinal)?;
            }
            Ok(crate::tui::TuiAction::Resume) => {
                return Err(anyhow::anyhow!("resume requested while capture was active"));
            }
            Ok(crate::tui::TuiAction::SwitchDevice(index)) => {
                let mut devices = crate::recorder::list_input_devices();
                if index >= devices.len() {
                    return Err(anyhow::anyhow!(
                        "selected audio input is no longer available"
                    ));
                }
                let (device_name, device) = devices.swap_remove(index);
                app.current_mic_name = device_name;
                selected_device = Some(device);
                ordinal = margins_store::canonical::next_segment_index(margins_dir, session_name)?;
                audio_path = margins_dir.join(format!("{session_name}_seg{ordinal}.wav"));
                meeting.open(ordinal)?;
            }
        }
    }

    meeting.finish((Local::now() - started_at).num_milliseconds().max(0) as u64)?;

    // Signal terminal decoding before asking what to do next. The rolling
    // worker continues draining queued live audio while the user decides.
    let mut live_finalizer = live.map(|worker| worker.begin_finish(live_timeline_duration_ms));

    // Save memo regardless of transcription result.
    let memo_result = app
        .save()
        .with_context(|| format!("could not save memo {}", &app.output_path));
    if let Err(error) = memo_result {
        if let Some(finalizer) = live_finalizer.take() {
            let _ = finalizer.complete();
        }
        return Err(error);
    }

    let action_result = if tui_error.is_none() {
        choose_post_capture_action()
    } else {
        Ok(PostCaptureAction::NotOffered)
    };
    let show_progress = matches!(
        &action_result,
        Ok(PostCaptureAction::Distill | PostCaptureAction::SavedOnly)
    );
    let distill_requested = matches!(&action_result, Ok(PostCaptureAction::Distill));
    let live_result = (|| -> Result<()> {
        if let Some(finalizer) = live_finalizer.take() {
            let completed = if show_progress {
                finalizer.wait_with_spinner(distill_requested)?
            } else {
                finalizer.complete()?
            };
            if completed {
                meeting.register_transcript(live_artifact_ordinal)?;
            }
        }
        Ok(())
    })();

    if let Some(error) = tui_error {
        return Err(error);
    }
    let action = action_result?;
    live_result?;
    Ok(action)
}

#[cfg(feature = "audio-capture")]
fn resolve_artifact(work_dir: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        work_dir.join(path)
    }
}
