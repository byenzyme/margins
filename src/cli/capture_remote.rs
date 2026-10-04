#[cfg(any(test, feature = "audio-capture"))]
fn cleanup_failed_remote_recorder_start<F>(
    mut transfer: margins_workflows::remote_workspace::NativeRemoteTransfer,
    ended_at_ms: u64,
    reservation_intent: &mut Option<
        margins_workflows::remote_workspace::CaptureReservationIntentV1,
    >,
    transfer_dir: &Path,
    uploader_done: &AtomicBool,
    uploader: std::thread::JoinHandle<()>,
    deliver: F,
) -> Vec<String>
where
    F: FnOnce(&mut margins_workflows::remote_workspace::DurableTransferSpool) -> Result<()>,
{
    let mut errors = Vec::new();
    let abort_sealed = match transfer.seal_session(
        ended_at_ms,
        margins_meeting_protocol::SessionFinalizeReasonV1::Error,
    ) {
        Ok(_) => true,
        Err(error) => {
            errors.push(format!("could not persist abort intent: {error}"));
            false
        }
    };
    if let Some(intent) = reservation_intent.take() {
        if let Err(error) = intent.remove(transfer_dir) {
            errors.push(format!("could not dispose capture reservation: {error}"));
        }
    }
    uploader_done.store(true, Ordering::Release);
    if uploader.join().is_err() {
        errors.push("remote delivery worker panicked during abort".into());
    }
    let mut spool = transfer.into_spool();
    if abort_sealed {
        if let Err(error) = deliver(&mut spool) {
            errors.push(format!(
                "abort delivery remains pending in transfer {}: {error}",
                spool.manifest().transfer_id
            ));
        }
    }
    errors
}

#[cfg(feature = "audio-capture")]
fn copy_remote_recovery_for_local_asr(
    source: &Path,
    directory: &Path,
    transfer_id: &str,
    segment_index: usize,
) -> Result<std::path::PathBuf> {
    use std::fs::OpenOptions;
    use std::io::copy;

    if !directory.is_absolute() {
        bail!("local audio directory must be absolute");
    }
    std::fs::create_dir_all(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        let name = format!("{transfer_id}-seg{segment_index}.wav");
        let destination = directory.join(name);
        let temporary = destination.with_extension("wav.partial");
        let mut input = std::fs::File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        copy(&mut input, &mut output)?;
        output.sync_all()?;
        std::fs::rename(&temporary, &destination)?;
        return Ok(destination);
    }
    #[cfg(not(unix))]
    {
        let destination = directory.join(format!("{transfer_id}-seg{segment_index}.wav"));
        let mut input = std::fs::File::open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)?;
        copy(&mut input, &mut output)?;
        output.sync_all()?;
        Ok(destination)
    }
}

#[cfg(feature = "audio-capture")]
fn run_remote_native_capture(
    remote: &str,
    workspace_id: &str,
    command: &Option<Command>,
    controller: Option<native_bridge::CaptureController>,
    local_audio_dir: Option<&Path>,
    mic_device_name: Option<&str>,
    prepared_connection: Option<margins_workflows::remote_workspace::RemoteConnection>,
    permissions_verified: bool,
) -> Result<()> {
    use margins_meeting_protocol::{
        SegmentCloseReasonV1, SessionFinalizeReasonV1, WorkspaceAttachV1, WorkspaceMemoLineV1,
        WorkspaceMemoReplaceV1,
    };
    use margins_workflows::remote_workspace::{
        deliver_available, deliver_transfer, list_transfers, native_create_session_command,
        pending_capture_reservations, transfer_root, validate_native_opus_capture_lanes,
        CaptureReservationIntentV1, CaptureReservationRequestV1, DurableTransferSpool,
        NativeRemoteLane, NativeRemoteTransfer, RemoteConnection, NATIVE_REMOTE_RATE_HZ,
    };

    if !permissions_verified {
        ensure_capture_permissions(&NativeCapturePermissionSource)?;
    }
    let mut selected_device = mic_device_name
        .map(|name| {
            crate::recorder::list_input_devices()
                .into_iter()
                .find(|(available, _)| available == name)
                .map(|(name, device)| crate::recorder::SelectedInputDevice {
                    uid: crate::recorder::input_device_uid_at(&name, 0),
                    name,
                    occurrence: 0,
                    device,
                })
                .with_context(|| format!("microphone input device not found: {name}"))
        })
        .transpose()?;
    let mut preference_note = None;
    // The Menu owns its input selection. Its unpinned mode means the system
    // default, regardless of a separate TUI preference.
    let preference = match if controller.is_some() {
        Ok(None)
    } else {
        audio_preferences::load()
    } {
        Ok(preference) => preference,
        Err(error) => {
            preference_note = Some(if mic_device_name.is_some() {
                format!("Could not read saved mic choice ({error}); using requested input")
            } else {
                format!("Could not read saved mic choice ({error}); using system default")
            });
            None
        }
    };
    if selected_device.is_none() {
        let (preferred_device, note) = audio_preferences::resolve(preference.as_ref())?;
        selected_device = preferred_device;
        preference_note = note.or(preference_note);
    }
    let mut saved_choice_active =
        controller.is_none() && mic_device_name.is_none() && selected_device.is_some();
    let mut pending_selection = false;
    let token = std::env::var("MARGINS_REMOTE_TOKEN").ok();
    let connection = match prepared_connection {
        Some(connection) => connection,
        None => RemoteConnection::connect(remote, workspace_id, token.as_deref())?,
    };
    let capabilities = &connection.capabilities;
    let opus_supported = capabilities.capture_formats.iter().any(|format| {
        format.codec == margins_meeting_protocol::AudioCodecV1::Opus
            && format.container == margins_meeting_protocol::AudioContainerV1::PacketStream
            && format.sample_rate_hz == NATIVE_REMOTE_RATE_HZ
            && format.channel_count == 1
    });
    if !opus_supported {
        bail!("remote instance does not advertise native mono Opus packet-stream capture");
    }

    let transfer_dir = transfer_root()?;
    std::fs::create_dir_all(&transfer_dir)?;
    let mut reservation_intent;
    let (spool, session_id, initial_offset_ms) = match command {
        Some(Command::New { title }) => {
            let pending = pending_capture_reservations(&transfer_dir)?
                .into_iter()
                .filter(|intent| {
                    intent.instance_id == capabilities.instance_id.as_ref()
                        && intent.remote_url == remote
                        && intent.workspace_id == workspace_id
                        && matches!(&intent.request, CaptureReservationRequestV1::Create { command }
                            if matches!(&command.body, margins_meeting_protocol::ClientMessageBodyV1::CreateSession(create) if &create.title == title))
                })
                .collect::<Vec<_>>();
            if pending.len() > 1 {
                bail!("multiple unfinished remote session reservations match this command; inspect the transfer directory before retrying");
            }
            let intent = if let Some(intent) = pending.into_iter().next() {
                intent
            } else {
                let session_id = format!(
                    "remote-{}-{}",
                    Local::now().format("%Y-%m-%d-%H-%M-%S"),
                    &uuid::Uuid::new_v4().simple().to_string()[..8]
                );
                let transfer_id = uuid::Uuid::new_v4().to_string();
                let command = native_create_session_command(
                    &session_id,
                    &format!("reserve-{transfer_id}"),
                    title.clone(),
                    "margins-native-cli",
                );
                CaptureReservationIntentV1 {
                    schema: "margins.capture-reservation.v1".into(),
                    transfer_id,
                    instance_id: capabilities.instance_id.as_ref().into(),
                    remote_url: remote.into(),
                    workspace_id: workspace_id.into(),
                    session_id,
                    request: CaptureReservationRequestV1::Create { command },
                }
                .persist(&transfer_dir)?
            };
            let spool = remote_spool_from_reservation(
                &connection,
                &capabilities,
                &transfer_dir,
                remote,
                workspace_id,
                &intent,
            )?;
            let session_id = intent.session_id.clone();
            reservation_intent = Some(intent);
            (spool, session_id, 0)
        }
        Some(Command::Attach { session }) => {
            let requested =
                match session {
                    Some(session) => session.clone(),
                    None => connection
                        .client
                        .current()?
                        .context(
                            "no current remote capture is selected for this client and Workspace",
                        )?
                        .0,
                };
            let target_summary = connection
                .client
                .session_summary(&requested)?
                .context("remote session was not found in the selected Workspace")?;
            validate_native_opus_capture_lanes(&target_summary.capture_lanes)?;
            let mut found = None;
            for transfer_id in list_transfers(&transfer_dir)? {
                let candidate = DurableTransferSpool::open(
                    &transfer_dir,
                    &transfer_id,
                    capabilities.limits.spool_reserve_bytes,
                )?;
                let manifest = candidate.manifest();
                if manifest.instance_id == capabilities.instance_id.as_ref()
                    && manifest.workspace_id == workspace_id
                    && manifest.session_id == requested
                {
                    found = Some(candidate);
                    break;
                }
            }
            if let Some(spool) = found {
                if spool.manifest().finalize_command.is_some() {
                    bail!(
                        "remote transfer is already sealed; retry delivery instead of attaching audio"
                    );
                }
                reservation_intent = pending_capture_reservations(&transfer_dir)?
                    .into_iter()
                    .find(|intent| intent.transfer_id == spool.manifest().transfer_id);
                let offset = spool
                    .manifest()
                    .close_commands
                    .iter()
                    .filter_map(|command| match &command.body {
                        margins_meeting_protocol::ClientMessageBodyV1::CloseSegment(close) => {
                            Some(close.ended_at_ms.0)
                        }
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0);
                (spool, requested, offset)
            } else {
                let pending = pending_capture_reservations(&transfer_dir)?
                    .into_iter()
                    .filter(|intent| {
                        intent.instance_id == capabilities.instance_id.as_ref()
                            && intent.remote_url == remote
                            && intent.workspace_id == workspace_id
                            && intent.session_id == requested
                            && matches!(intent.request, CaptureReservationRequestV1::Attach { .. })
                    })
                    .collect::<Vec<_>>();
                if pending.len() > 1 {
                    bail!("multiple unfinished attach reservations match this session; inspect the transfer directory before retrying");
                }
                if let Some(intent) = pending.into_iter().next() {
                    let spool = remote_spool_from_reservation(
                        &connection,
                        &capabilities,
                        &transfer_dir,
                        remote,
                        workspace_id,
                        &intent,
                    )?;
                    reservation_intent = Some(intent);
                    let offset = match &reservation_intent.as_ref().unwrap().request {
                        CaptureReservationRequestV1::Attach { request } => request.started_at_ms.0,
                        _ => unreachable!(),
                    };
                    (spool, requested, offset)
                } else {
                    if !target_summary.input_finalized {
                        bail!(
                            "remote session has an active producer; recover it from its owning client"
                        );
                    }
                    let offset = target_summary
                        .capture_duration_ms
                        .context("remote session lacks a durable capture boundary")?
                        .0;
                    let transfer_id = uuid::Uuid::new_v4().to_string();
                    let attach = WorkspaceAttachV1 {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        prior_finalize_message_id: target_summary
                            .capture_finalize_message_id
                            .context("remote session lacks a durable finalize identity")?,
                        requested_at_unix_ms: margins_meeting_protocol::UnixMillis(
                            Local::now().timestamp_millis().max(0) as u64,
                        ),
                        started_at_ms: margins_meeting_protocol::SessionMillis(offset),
                    };
                    let intent = CaptureReservationIntentV1 {
                        schema: "margins.capture-reservation.v1".into(),
                        transfer_id,
                        instance_id: capabilities.instance_id.as_ref().into(),
                        remote_url: remote.into(),
                        workspace_id: workspace_id.into(),
                        session_id: requested.clone(),
                        request: CaptureReservationRequestV1::Attach { request: attach },
                    }
                    .persist(&transfer_dir)?;
                    let spool = remote_spool_from_reservation(
                        &connection,
                        &capabilities,
                        &transfer_dir,
                        remote,
                        workspace_id,
                        &intent,
                    )?;
                    reservation_intent = Some(intent);
                    (spool, requested, offset)
                }
            }
        }
        _ => bail!("remote native capture requires new or attach"),
    };
    let _capture_lease = spool.acquire_capture_lease()?;

    // Recovery establishes the only truthful media boundary for the next
    // generation. Do this before constructing the memo clock or capture origin;
    // otherwise a recovered interrupted segment can overlap the new one.
    let mut transfer = NativeRemoteTransfer::new(spool);
    transfer.recover_interrupted_segment(SegmentCloseReasonV1::Error)?;
    let initial_offset_ms = transfer
        .last_closed_ended_at_ms()
        .unwrap_or(initial_offset_ms)
        .max(initial_offset_ms);

    // The CoreML worker is optional. Its checkpoint stays inside this scoped
    // transfer until a separate publisher sends a bounded copy to the service.
    let checkpoint_path = transfer.spool().root().join("live-checkpoint.json");
    let live_status = Arc::new(AtomicU8::new(crate::app::LIVE_TRANSCRIPTION_WARMING));
    let live = start_live_transcript_worker(
        checkpoint_path.clone(),
        initial_offset_ms,
        live_status.clone(),
    );
    let checkpoint_publisher = if live.is_some() {
        let client = connection.client.clone();
        let session = session_id.clone();
        let producer_token = transfer.spool().producer_token()?;
        let done = Arc::new(AtomicBool::new(false));
        let thread_done = done.clone();
        let join = std::thread::Builder::new()
            .name("margins-remote-live-checkpoint".into())
            .spawn(move || {
                publish_remote_live_checkpoints(
                    &client,
                    &session,
                    &producer_token,
                    &checkpoint_path,
                    &thread_done,
                );
            })?;
        Some(RemoteCheckpointPublisher {
            done,
            join: Some(join),
        })
    } else {
        None
    };

    let initial_memo = connection.client.memo(&session_id)?;
    let initial_revision = initial_memo.revision.clone();
    let initial_lines = initial_memo.lines.clone();
    let started_at = Local::now()
        - chrono::Duration::milliseconds(initial_offset_ms.min(i64::MAX as u64) as i64);
    let draft_path = transfer.spool().root().join("memo-draft.md");
    let document = if draft_path.is_file() {
        margins_core::TimedMemoDocument::parse_markdown(&std::fs::read_to_string(&draft_path)?)
    } else {
        margins_core::TimedMemoDocument::from_committed(
            initial_memo
                .lines
                .iter()
                .cloned()
                .map(|line| margins_core::TimedMemoLine {
                    text: line.text,
                    created_secs: line.created_secs,
                    edited_secs: line.edited_secs,
                    draft_started_secs: line.draft_started_secs,
                    audio_pending_at_mark: line.audio_pending_at_mark,
                    block_ordinal: line.block_ordinal,
                })
                .collect(),
        )
    };
    let mic_name = crate::recorder::default_input_device_name().unwrap_or_else(|| "Unknown".into());
    let mut app = crate::app::App::from_memo(
        document,
        draft_path.to_string_lossy().into_owned(),
        started_at,
        mic_name,
    );
    app.preferred_mic_name = preference.as_ref().map(|choice| choice.name.clone());
    app.preferred_mic_uid = preference.as_ref().and_then(|choice| choice.uid.clone());
    app.message = preference_note;
    let uploader_done = Arc::new(AtomicBool::new(false));
    let uploader_state = Arc::new(AtomicU8::new(crate::app::REMOTE_DELIVERY_CURRENT));
    let uploader_pending_chunks = Arc::new(AtomicU64::new(0));
    let uploader_pending_bytes = Arc::new(AtomicU64::new(0));
    app.remote_delivery_state = uploader_state.clone();
    app.remote_pending_chunks = uploader_pending_chunks.clone();
    app.remote_pending_bytes = uploader_pending_bytes.clone();
    app.live_transcription_status = live_status;
    let uploader_parent = transfer
        .spool()
        .root()
        .parent()
        .context("remote transfer has no parent")?
        .to_path_buf();
    let uploader_id = transfer.spool().manifest().transfer_id.clone();
    let uploader_client = connection.client.clone();
    let uploader_reserve = capabilities.limits.spool_reserve_bytes;
    let uploader_batch_limit = capabilities.limits.max_in_flight_chunks.max(1) as usize;
    let uploader_done_flag = uploader_done.clone();
    let uploader = std::thread::Builder::new()
        .name("margins-remote-delivery".into())
        .spawn(move || {
            let mut backoff = std::time::Duration::from_millis(50);
            loop {
                if uploader_done_flag.load(Ordering::Acquire) {
                    break;
                }
                let result = (|| -> Result<(bool, bool)> {
                    let mut spool = DurableTransferSpool::open(
                        &uploader_parent,
                        &uploader_id,
                        uploader_reserve,
                    )?;
                    let chunks = spool.pending_chunks()?;
                    uploader_pending_chunks.store(chunks.len() as u64, Ordering::Release);
                    uploader_pending_bytes.store(
                        chunks.iter().map(|chunk| chunk.size_bytes).sum(),
                        Ordering::Release,
                    );
                    let has_work = !chunks.is_empty() || !spool.pending_closes().is_empty();
                    let catching_up = chunks.len() >= uploader_batch_limit;
                    if has_work {
                        deliver_available(&mut spool, &uploader_client)?;
                    }
                    Ok((has_work, catching_up))
                })();
                match result {
                    Ok((_, catching_up)) => {
                        uploader_state
                            .store(crate::app::REMOTE_DELIVERY_CURRENT, Ordering::Release);
                        // A full batch means durable ingress is outrunning the
                        // last request. Continue promptly until below the
                        // advertised catch-up threshold; partial/idle polling
                        // remains relaxed and never busy-spins.
                        backoff = if catching_up {
                            std::time::Duration::from_millis(1)
                        } else {
                            std::time::Duration::from_millis(50)
                        };
                    }
                    Err(error) => {
                        uploader_state
                            .store(crate::app::REMOTE_DELIVERY_PENDING, Ordering::Release);
                        if remote_delivery_requires_credentials(&error) {
                            // Preserve the spool and stop background retries until
                            // the user repairs/reissues credentials. Final Stop
                            // still releases devices first and makes one truthful
                            // delivery attempt before returning the transfer id.
                            break;
                        }
                        backoff = (backoff * 2).min(std::time::Duration::from_secs(2));
                    }
                }
                std::thread::sleep(backoff);
            }
        })?;
    let capture_started = std::time::Instant::now();
    let mut announced_sources = false;
    let mut local_segment_index = 0usize;
    'capture: loop {
        let offset_ms =
            initial_offset_ms.saturating_add(capture_started.elapsed().as_millis() as u64);
        let segment_id = format!("native-{}", uuid::Uuid::new_v4().simple());
        let stop = Arc::new(AtomicBool::new(false));
        let queued_samples = Arc::new(AtomicU64::new(0));
        let (sender, receiver) = mpsc::channel();
        let sink = crate::recorder::LiveAudioSink {
            sender,
            generation: 1,
            generation_clock: Arc::new(Mutex::new(crate::recorder::LiveGenerationClock {
                generation: 1,
                session_offset_ms: offset_ms,
            })),
            mic_accepted_samples: Arc::new(AtomicU64::new(0)),
            system_accepted_samples: Arc::new(AtomicU64::new(0)),
            mic_dropped_samples: Arc::new(AtomicU64::new(0)),
            system_dropped_samples: Arc::new(AtomicU64::new(0)),
            queued_samples: queued_samples.clone(),
            queue_max_samples: u64::from(NATIVE_REMOTE_RATE_HZ) * 10 * 2,
        };
        let recorder = match audio_preferences::open_with_saved_fallback(
            saved_choice_active.then_some(preference.as_ref()).flatten(),
            selected_device.as_ref(),
            |choice| {
                crate::recorder::RecorderHandle::start_with_selected_audio(
                    stop.clone(),
                    choice,
                    Some(sink.clone()),
                )
            },
        ) {
            Ok((recorder, fell_back, note)) => {
                if fell_back {
                    selected_device = None;
                    saved_choice_active = false;
                }
                if let Some(note) = note {
                    app.message = Some(note);
                }
                recorder
            }
            Err(error) => {
                // A reservation is not a successful capture. Seal it aborted,
                // retaining the transfer if the server cannot acknowledge. No
                // cleanup step may short-circuit the later steps: in particular,
                // stop/join the uploader and dispose the reservation even if the
                // durable abort intent itself fails.
                let cleanup_errors = cleanup_failed_remote_recorder_start(
                    transfer,
                    initial_offset_ms,
                    &mut reservation_intent,
                    &transfer_dir,
                    &uploader_done,
                    uploader,
                    |spool| deliver_transfer(spool, &connection.client),
                );
                if cleanup_errors.is_empty() {
                    return Err(error);
                }
                return Err(error.context(cleanup_errors.join("; ")));
            }
        };
        app.current_mic_name = recorder.mic_name().to_owned();
        app.current_mic_uid = recorder.mic_uid().map(str::to_owned);
        if pending_selection {
            if let Some(selected) = selected_device.as_ref() {
                audio_preferences::remember_selection(&mut app, selected);
            }
            pending_selection = false;
        }
        app.mic_silent = false;
        app.suggested_mic_name = None;
        app.devices = crate::recorder::list_input_devices()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        app.device_uids = crate::recorder::input_device_uid_snapshot(&app.devices);
        if !announced_sources {
            eprintln!(
                "Recording on this Mac · Saving to {} / {} · Microphone + system audio enabled",
                capabilities.instance_id.as_ref(),
                workspace_id
            );
            announced_sources = true;
        }
        transfer.begin_segment(segment_id.clone(), offset_ms)?;
        if let Some(intent) = reservation_intent.take() {
            intent.remove(&transfer_dir)?;
        }
        let recovery_path = transfer.spool().recovery_path(&segment_id)?;
        let active_transfer_id = transfer.spool().manifest().transfer_id.clone();
        let worker_stop = stop.clone();
        let live_sink = live.as_ref().map(|worker| {
            let mut sink = worker.sink_for_offset(offset_ms);
            // Native devices can deliver 48 kHz on both lanes. Bound the
            // unresampled queue to roughly thirty seconds of those samples.
            sink.queue_max_samples = 48_000 * 30 * 2;
            sink
        });
        let worker = std::thread::Builder::new()
            .name("margins-remote-spool".into())
            .spawn(move || {
                let mut transfer = transfer;
                let result = (|| -> Result<()> {
                    while let Ok(chunk) = receiver.recv() {
                        let count = chunk.samples.len() as u64;
                        let lane = match chunk.channel {
                            crate::recorder::LiveAudioChannel::Mic => NativeRemoteLane::Microphone,
                            crate::recorder::LiveAudioChannel::System => NativeRemoteLane::System,
                        };
                        let append = transfer.append_f32(lane, chunk.sample_rate, &chunk.samples);
                        queued_samples.fetch_sub(count, Ordering::Relaxed);
                        append?;
                        if let Some(sink) = &live_sink {
                            enqueue_remote_live_chunk(sink, &chunk);
                        }
                    }
                    Ok(())
                })();
                if result.is_err() {
                    worker_stop.store(true, Ordering::SeqCst);
                }
                (transfer, result)
            })?;

        app.mic_level = recorder.mic_peak();
        app.spk_level = recorder.spk_peak();
        app.mic_drops = recorder.mic_drops();
        app.spk_drops = recorder.spk_drops();
        app.mic_frames = recorder.mic_frames();
        app.mic_silence = recorder.mic_silence();
        app.mic_rate = recorder.mic_rate();
        app.mic_real_spool_frames = recorder.real_spool_frames(crate::recorder::CaptureLane::Mic);
        app.spk_real_spool_frames =
            recorder.real_spool_frames(crate::recorder::CaptureLane::System);
        app.mic_no_audio_received.store(false, Ordering::Release);
        app.spk_no_audio_received.store(false, Ordering::Release);
        app.spk_silence = recorder.spk_silence();
        app.spk_frames = recorder.spk_frames();
        app.spk_rate = recorder.spk_rate();
        if let Some(controller) = &controller {
            controller.recording(&session_id, &active_transfer_id, &sink, &recorder);
        }
        // The recorder owns the sender from here. Keeping this clone alive
        // would prevent the spool worker's receive loop from closing on stop.
        drop(sink);
        let action = if let Some(controller) = &controller {
            controller.wait_action(&stop)?
        } else {
            crate::tui::run_tui(&mut app, stop.clone())
                .map_err(|error| anyhow::anyhow!(error.to_string()))?
        };
        // run_tui set the stop flag before returning. This call synchronously
        // retires both native devices before memo/network/ASR work begins.
        recorder.stop_and_write(&recovery_path.to_string_lossy())?;
        if let Some(directory) = local_audio_dir {
            match copy_remote_recovery_for_local_asr(
                &recovery_path,
                directory,
                &active_transfer_id,
                local_segment_index,
            ) {
                Ok(path) => {
                    if let Some(controller) = &controller {
                        controller.local_audio_saved(&path);
                    }
                }
                Err(error) => {
                    if let Some(controller) = &controller {
                        controller.local_audio_failed(&format!("{error:#}"));
                    }
                }
            }
            local_segment_index += 1;
        }
        let (returned, spool_result) = worker
            .join()
            .map_err(|_| anyhow::anyhow!("remote spool worker panicked"))?;
        transfer = returned;
        spool_result.context("remote audio spool failed; local recovery WAV was retained")?;

        match action {
            crate::tui::TuiAction::Pause => {
                transfer.close_segment(SegmentCloseReasonV1::Pause)?;
                app.set_capture_paused(true);
                if let Some(controller) = &controller {
                    controller.paused();
                }
                app.mic_level = Arc::new(AtomicU32::new(0));
                app.spk_level = Arc::new(AtomicU32::new(0));
                loop {
                    let paused_action = if let Some(controller) = &controller {
                        controller.wait_paused_action()?
                    } else {
                        crate::tui::run_tui(&mut app, Arc::new(AtomicBool::new(false)))
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?
                    };
                    match paused_action {
                        crate::tui::TuiAction::Resume => {
                            app.set_capture_paused(false);
                            break;
                        }
                        crate::tui::TuiAction::Quit => break 'capture,
                        crate::tui::TuiAction::SwitchDevice(index) => {
                            let selected = crate::recorder::selected_input_device(
                                &app.devices,
                                &app.device_uids,
                                index,
                            )?;
                            selected_device = Some(selected);
                            saved_choice_active = false;
                            pending_selection = true;
                        }
                        crate::tui::TuiAction::Pause => {}
                    }
                }
            }
            crate::tui::TuiAction::SwitchDevice(index) => {
                transfer.close_segment(SegmentCloseReasonV1::Rollover)?;
                let selected =
                    crate::recorder::selected_input_device(&app.devices, &app.device_uids, index)?;
                selected_device = Some(selected);
                saved_choice_active = false;
                pending_selection = true;
            }
            crate::tui::TuiAction::Quit => {
                transfer.close_segment(SegmentCloseReasonV1::Stop)?;
                break 'capture;
            }
            crate::tui::TuiAction::Resume => bail!("resume requested while capture was active"),
        }
    }
    // Pin the media boundary before any uploader join, memo write, or server
    // work. Stop drain latency must never inflate recorded duration.
    let final_ended_at_ms = transfer
        .last_closed_ended_at_ms()
        .context("remote capture stopped without a durable media boundary")?;
    if let Some(controller) = &controller {
        controller.saving();
    }
    if let Some(worker) = live {
        let _ = worker
            .begin_finish(final_ended_at_ms.saturating_sub(initial_offset_ms))
            .complete();
    }
    drop(checkpoint_publisher);
    uploader_done.store(true, Ordering::Release);
    uploader
        .join()
        .map_err(|_| anyhow::anyhow!("remote delivery worker panicked"))?;
    app.save().context("remote memo draft could not be saved")?;
    let lines: Vec<WorkspaceMemoLineV1> = app
        .memo
        .lines()
        .iter()
        .cloned()
        .map(|line| WorkspaceMemoLineV1 {
            text: line.text,
            created_secs: line.created_secs,
            edited_secs: line.edited_secs,
            draft_started_secs: line.draft_started_secs,
            audio_pending_at_mark: line.audio_pending_at_mark,
            block_ordinal: line.block_ordinal,
        })
        .collect();
    // The native bridge has no memo editor. BB/Codex may have edited the
    // Workspace memo during capture, so sending our initial snapshot here
    // would overwrite it (or block finalization with a revision conflict).
    if controller.is_none() && remote_memo_was_edited(&initial_lines, &lines) {
        let memo_request_id = format!("native-memo-{}", transfer.spool().manifest().transfer_id);
        transfer
            .spool_mut()
            .set_memo_intent(WorkspaceMemoReplaceV1 {
                request_id: memo_request_id,
                expected_revision: initial_revision,
                lines,
            })?;
    }
    transfer.seal_session(final_ended_at_ms, SessionFinalizeReasonV1::Completed)?;
    let transfer_id = transfer.spool().manifest().transfer_id.clone();
    let mut spool = transfer.into_spool();
    match deliver_transfer(&mut spool, &connection.client) {
        Ok(()) => {
            eprintln!("Saved to remote Workspace; processing state is separate.");
            Ok(())
        }
        Err(error) => bail!(
            "recording stopped; upload pending in transfer {transfer_id}. Retry with `margins transfers retry {transfer_id}`: {error}"
        ),
    }
}

#[cfg(feature = "audio-capture")]
fn remote_memo_was_edited(
    initial: &[margins_meeting_protocol::WorkspaceMemoLineV1],
    final_lines: &[margins_meeting_protocol::WorkspaceMemoLineV1],
) -> bool {
    // App::from_memo appends one empty TUI draft even if nobody types. A
    // capture-only session must not turn that draft into a remote replacement.
    let actual = if final_lines.len() == initial.len() + 1
        && final_lines
            .last()
            .is_some_and(|line| line.text.trim().is_empty())
    {
        &final_lines[..initial.len()]
    } else {
        final_lines
    };
    actual != initial
}

#[cfg(feature = "audio-capture")]
fn remote_spool_from_reservation(
    connection: &margins_workflows::remote_workspace::RemoteConnection,
    capabilities: &margins_meeting_protocol::WorkspaceCapabilitiesV1,
    transfer_dir: &Path,
    remote: &str,
    workspace_id: &str,
    intent: &margins_workflows::remote_workspace::CaptureReservationIntentV1,
) -> Result<margins_workflows::remote_workspace::DurableTransferSpool> {
    use margins_workflows::remote_workspace::{
        promote_capture_reservation, CaptureReservationRequestV1,
    };
    if capabilities.instance_id.as_ref() != intent.instance_id
        || intent.remote_url != remote
        || intent.workspace_id != workspace_id
    {
        bail!("reservation intent does not match the selected remote instance and Workspace");
    }
    let spool = promote_capture_reservation(
        intent,
        transfer_dir,
        capabilities.limits.spool_reserve_bytes,
        || {
            let reservation = match &intent.request {
                CaptureReservationRequestV1::Create { command } => {
                    connection.client.reserve(command)?
                }
                CaptureReservationRequestV1::Attach { request } => {
                    connection.client.attach(&intent.session_id, request)?
                }
            };
            Ok(reservation.producer_token)
        },
    )?;
    if spool.manifest().finalize_command.is_some() {
        bail!("reserved remote transfer is already sealed; retry its delivery instead of starting capture");
    }
    Ok(spool)
}

#[cfg(feature = "audio-capture")]
fn remote_delivery_requires_credentials(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    [
        "unauthorized:",
        "forbidden:",
        "credential is invalid",
        "credential is revoked or expired",
        "producer token",
        "producer_token",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

#[cfg(feature = "audio-capture")]
struct RemoteCheckpointPublisher {
    done: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[cfg(feature = "audio-capture")]
impl Drop for RemoteCheckpointPublisher {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(feature = "audio-capture")]
fn enqueue_remote_live_chunk(
    sink: &crate::recorder::LiveAudioSink,
    chunk: &crate::recorder::LiveAudioChunk,
) {
    use crate::recorder::LiveAudioChannel;
    let count = chunk.samples.len() as u64;
    let dropped = match chunk.channel {
        LiveAudioChannel::Mic => &sink.mic_dropped_samples,
        LiveAudioChannel::System => &sink.system_dropped_samples,
    };
    let accepted = match chunk.channel {
        LiveAudioChannel::Mic => &sink.mic_accepted_samples,
        LiveAudioChannel::System => &sink.system_accepted_samples,
    };
    let reserved = sink
        .queued_samples
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
            let next = queued.checked_add(count)?;
            (next <= sink.queue_max_samples).then_some(next)
        })
        .is_ok();
    if !reserved {
        dropped.fetch_add(count, Ordering::Relaxed);
        return;
    }
    let mut copy = chunk.clone();
    copy.generation = sink.generation;
    if sink.sender.send(copy).is_ok() {
        accepted.fetch_add(count, Ordering::Relaxed);
    } else {
        sink.queued_samples.fetch_sub(count, Ordering::AcqRel);
        dropped.fetch_add(count, Ordering::Relaxed);
    }
}

#[cfg(feature = "audio-capture")]
fn publish_remote_live_checkpoints(
    client: &margins_workflows::remote_workspace::WorkspaceHttpClient,
    session: &str,
    producer_token: &str,
    path: &Path,
    done: &AtomicBool,
) {
    const MAX_BYTES: u64 = 256 * 1024;
    let mut last_sent = Vec::new();
    let mut last_attempt = std::time::Instant::now() - std::time::Duration::from_secs(3);
    loop {
        let closing = done.load(Ordering::Acquire);
        if closing || last_attempt.elapsed() >= std::time::Duration::from_secs(3) {
            if let Ok(metadata) = std::fs::metadata(path) {
                if metadata.len() > 0 && metadata.len() <= MAX_BYTES {
                    if let Ok(body) = std::fs::read(path) {
                        if body != last_sent {
                            last_attempt = std::time::Instant::now();
                            match client.put_live_checkpoint(session, producer_token, body.clone())
                            {
                                Ok(()) => last_sent = body,
                                Err(error) => crate::cli_log::event(
                                    "remote_live_checkpoint_upload_failed",
                                    crate::cli_log::error_summary(&error),
                                ),
                            }
                        }
                    }
                }
            }
        }
        if closing {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}
