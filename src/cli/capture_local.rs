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

#[cfg(feature = "audio-capture")]
struct SegmentOutcome {
    action: PostCaptureAction,
    error: Option<anyhow::Error>,
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
    let owner = capture_local_runtime::SessionOwnerLock::acquire(&margins_dir, &name)?;
    // Silent bookkeeping so desktop and `recent --all` can enumerate this folder.
    margins_workflows::project::register_vault_silently(work_dir);
    let mut meeting = capture_local_runtime::LocalMeetingProducer::reserve(
        &margins_dir,
        &name,
        title,
        started_at,
    )?;
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

    let outcome = run_segment(
        &mut app,
        0,
        live,
        live_artifact_ordinal,
        started_at,
        initial_offset_ms,
        initial_recorder,
        initial_stop,
        &mut meeting,
    )?;
    drop(owner);
    complete_post_capture(outcome, Some(&name))
}

#[cfg(feature = "audio-capture")]
fn attach_native_session(work_dir: &Path, selected: Option<&str>) -> Result<()> {
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
    let owner = capture_local_runtime::SessionOwnerLock::acquire(&margins_dir, &name)?;
    ensure_speech_model(false)?;
    let meta = margins_store::canonical::get_session_meta(&margins_dir, &name)?;
    let started_at = margins_store::canonical::get_session_start_time(&margins_dir, &name)?;
    let memo_path = resolve_artifact(work_dir, &meta.notes_path);
    let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&margins_dir)?;
    let observed = authority.memo(&name)?;
    let parsed = margins_core::TimedMemoDocument::from_committed(observed.lines.clone());
    let offset_ms = (Local::now() - started_at).num_milliseconds().max(0);
    let mut meeting = capture_local_runtime::LocalMeetingProducer::recover(
        &margins_dir,
        &name,
        offset_ms as u64,
        meta.title.as_deref(),
        started_at,
        &owner,
    )?;
    if let Some((pending, offset)) = meeting.pending_segment()? {
        meeting.recover_pending_segment(pending, offset)?;
    }
    let ordinal = meeting.next_ordinal()?;
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

    let outcome = run_segment(
        &mut app,
        ordinal,
        live,
        live_artifact_ordinal,
        started_at,
        initial_offset_ms,
        initial_recorder,
        initial_stop,
        &mut meeting,
    )?;
    drop(owner);
    complete_post_capture(outcome, None)
}

#[cfg(feature = "audio-capture")]
fn complete_post_capture(outcome: SegmentOutcome, new_session: Option<&str>) -> Result<()> {
    let action_result = match outcome.action {
        PostCaptureAction::Distill => crate::note::run(false),
        PostCaptureAction::SavedOnly => {
            eprintln!("Session saved.");
            Ok(())
        }
        PostCaptureAction::NotOffered => {
            if outcome.error.is_none() {
                if let Some(name) = new_session {
                    eprintln!("{}", post_new_session_hint(name));
                }
            }
            Ok(())
        }
    };
    match (outcome.error, action_result) {
        (Some(capture), Err(action)) => Err(anyhow::anyhow!("{capture}; {action}")),
        (Some(capture), Ok(())) => Err(capture),
        (None, result) => result,
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "audio-capture")]
fn run_segment(
    app: &mut crate::app::App,
    initial_ordinal: i64,
    live: Option<LiveTranscriptWorker>,
    live_artifact_ordinal: i64,
    started_at: chrono::DateTime<Local>,
    initial_offset_ms: i64,
    initial_recorder: crate::recorder::RecorderHandle,
    initial_stop: Arc<AtomicBool>,
    meeting: &mut capture_local_runtime::LocalMeetingProducer,
) -> Result<SegmentOutcome> {
    let mut ordinal = initial_ordinal;
    let mut selected_device: Option<crate::recorder::InputDevice> = None;
    let mut live_timeline_duration_ms: u64 = 0;
    let mut initial_recorder = Some(initial_recorder);
    let mut initial_stop = Some(initial_stop);
    // Memo durability must not depend on the optional transcription path.
    let mut tui_error: Option<anyhow::Error> = None;
    let mut flush_failed = false;

    // Keep capture and resume exits inside this closure so the common memo
    // save and runtime finalization below always run.
    let capture_result = (|| -> Result<()> {
        loop {
            let segment_offset_ms = if initial_recorder.is_some() {
                initial_offset_ms
            } else {
                (Local::now() - started_at).num_milliseconds().max(0)
            };
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
            app.native_spool_overflow = match recorder.bound_native_spool() {
                Ok(overflow) => overflow,
                Err(error) => {
                    stop.store(true, Ordering::SeqCst);
                    let _ = recorder.stop_and_flush(|| Ok(()));
                    return Err(error.context("could not bound the native audio spool"));
                }
            };
            app.native_store_retrying = match meeting.start_stream(
                ordinal,
                segment_offset_ms as u64,
                recorder.native_spool_sources(),
            ) {
                Ok(retrying) => retrying,
                Err(error) => {
                    stop.store(true, Ordering::SeqCst);
                    let _ = recorder.stop_and_flush(|| Ok(()));
                    let recovery =
                        meeting.recover_failed_stream_and_close(ordinal, segment_offset_ms as u64);
                    return Err(match recovery {
                    Ok(()) => error.context("could not start runtime audio storage"),
                    Err(recovery_error) => anyhow::anyhow!(
                        "could not start runtime audio storage: {error:#}; recovery failed: {recovery_error:#}"
                    ),
                });
                }
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
            // Memo persistence never waits behind a runtime audio backlog.
            let _ = app
                .save()
                .with_context(|| format!("could not save memo {}", &app.output_path));

            let reason = if matches!(&tui_result, Ok(crate::tui::TuiAction::Pause)) {
                margins_meeting_protocol::SegmentCloseReasonV1::Pause
            } else if matches!(&tui_result, Ok(crate::tui::TuiAction::SwitchDevice(_))) {
                margins_meeting_protocol::SegmentCloseReasonV1::Rollover
            } else if tui_result.is_err() {
                margins_meeting_protocol::SegmentCloseReasonV1::Error
            } else {
                margins_meeting_protocol::SegmentCloseReasonV1::Stop
            };
            let mut runtime_duration_ms = 0;
            let flush_result = recorder.stop_and_flush(|| {
                runtime_duration_ms =
                    meeting.flush_stream_and_close(ordinal, segment_offset_ms as u64, reason)?;
                Ok(())
            });
            if let Err(error) = flush_result {
                let recovery =
                    meeting.recover_failed_stream_and_close(ordinal, segment_offset_ms as u64);
                runtime_duration_ms = meeting
                    .last_end_ms()
                    .saturating_sub(segment_offset_ms as u64);
                let recovery_note = recovery
                    .err()
                    .map(|error| format!("; recovery of committed audio also failed: {error:#}"))
                    .unwrap_or_default();
                tui_error = Some(anyhow::anyhow!(
                "audio writer failed: {error:#}{recovery_note}. Run margins attach to continue this session"
            ));
                flush_failed = true;
                live_timeline_duration_ms = live_timeline_duration_ms.max(
                    (segment_offset_ms as u64)
                        .saturating_sub(initial_offset_ms as u64)
                        .saturating_add(runtime_duration_ms),
                );
                break;
            }
            live_timeline_duration_ms = live_timeline_duration_ms.max(
                (segment_offset_ms as u64)
                    .saturating_sub(initial_offset_ms as u64)
                    .saturating_add(runtime_duration_ms),
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
                    ordinal = meeting.next_ordinal()?;
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
                    ordinal = meeting.next_ordinal()?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = capture_result {
        tui_error = Some(error.context("capture or resume failed; run margins attach to continue"));
    }

    finish_segment_after_capture(
        app,
        live,
        live_artifact_ordinal,
        started_at,
        live_timeline_duration_ms,
        meeting,
        tui_error,
        flush_failed,
    )
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "audio-capture")]
fn finish_segment_after_capture(
    app: &mut crate::app::App,
    live: Option<LiveTranscriptWorker>,
    live_artifact_ordinal: i64,
    started_at: chrono::DateTime<Local>,
    live_timeline_duration_ms: u64,
    meeting: &mut capture_local_runtime::LocalMeetingProducer,
    tui_error: Option<anyhow::Error>,
    flush_failed: bool,
) -> Result<SegmentOutcome> {
    // The paused editor can still receive edits after the active segment was
    // saved. A repeated save with an unresolved conflict reuses its draft.
    let memo_error = app
        .save()
        .with_context(|| format!("could not save memo {}", &app.output_path))
        .err();
    let finalization_error = meeting
        .finish_with_reason(
            (Local::now() - started_at).num_milliseconds().max(0) as u64,
            if tui_error.is_some() {
                margins_meeting_protocol::SessionFinalizeReasonV1::Error
            } else {
                margins_meeting_protocol::SessionFinalizeReasonV1::Completed
            },
        )
        .err();

    // Signal terminal decoding before asking what to do next. The rolling
    // worker continues draining queued live audio while the user decides.
    let mut live_finalizer = live.map(|worker| worker.begin_finish(live_timeline_duration_ms));

    let action_result = if tui_error.is_none() || flush_failed {
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

    let action = action_result?;
    let failures = [
        tui_error.map(|error| error.to_string()),
        memo_error.map(|error| error.to_string()),
        finalization_error.map(|error| format!("could not finalize session: {error:#}")),
        live_result
            .err()
            .map(|error| format!("live transcript failed: {error:#}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    Ok(SegmentOutcome {
        action,
        error: (!failures.is_empty()).then(|| anyhow::anyhow!(failures.join("; "))),
    })
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

#[cfg(all(test, feature = "audio-capture"))]
mod resume_failure_tests {
    use super::*;
    use margins_meeting_protocol::SegmentCloseReasonV1;

    #[test]
    fn resume_device_failure_saves_paused_edits_and_error_finalizes() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".margins");
        let started_at = Local::now();
        let mut meeting = capture_local_runtime::LocalMeetingProducer::reserve(
            &dir,
            "resume-failure",
            None,
            started_at,
        )
        .unwrap();
        let wav = dir.join("resume-failure_seg0.wav");
        let mut writer = hound::WavWriter::create(
            &wav,
            hound::WavSpec {
                channels: 2,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..1_600 {
            writer.write_sample(100_i16).unwrap();
            writer.write_sample(200_i16).unwrap();
        }
        writer.finalize().unwrap();
        meeting.open(0, 0).unwrap();
        meeting
            .ingest_wav_and_close(0, &wav, 0, SegmentCloseReasonV1::Pause)
            .unwrap();
        let authority = margins_store::SqliteWorkspaceAuthorityStorage::open(&dir).unwrap();
        let observed = authority.memo("resume-failure").unwrap();
        let mut app = crate::app::App::new(
            dir.join("resume-failure.md").to_string_lossy().into_owned(),
            started_at,
            "fake mic".into(),
        );
        app.bind_workspace_authority(dir.clone(), "resume-failure".into());
        app.observe_memo(observed.revision, observed.lines);
        app.set_capture_paused(true);
        for letter in "typed while paused".chars() {
            app.insert_char(letter);
        }
        let outcome = finish_segment_after_capture(
            &mut app,
            None,
            0,
            started_at,
            100,
            &mut meeting,
            Some(anyhow::anyhow!(
                "selected audio input is no longer available"
            )),
            false,
        )
        .unwrap();
        assert_eq!(outcome.action, PostCaptureAction::NotOffered);
        assert!(complete_post_capture(outcome, None)
            .unwrap_err()
            .to_string()
            .contains("selected audio input"));
        assert_eq!(
            authority.memo("resume-failure").unwrap().lines[0].text,
            "typed while paused"
        );
        assert_eq!(
            margins_store::canonical::list_sessions(&dir)
                .unwrap()
                .into_iter()
                .find(|session| session.name == "resume-failure")
                .unwrap()
                .lifecycle_state,
            "ended"
        );
    }
}
