use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

#[derive(Clone, serde::Serialize)]
pub struct SpeechSetupSnapshot {
    pub state: &'static str,
    pub message: String,
    pub progress: Option<f32>,
}

#[derive(Clone)]
pub struct SpeechSetup {
    snapshot: Arc<Mutex<SpeechSetupSnapshot>>,
    running: Arc<AtomicBool>,
}

impl SpeechSetup {
    pub fn new(ready: bool, supported: bool) -> Self {
        Self {
            snapshot: Arc::new(Mutex::new(SpeechSetupSnapshot {
                state: if ready {
                    "ready"
                } else if supported {
                    "preparing"
                } else {
                    "unavailable"
                },
                message: if ready {
                    "Transcription ready"
                } else if supported {
                    "Preparing transcription model"
                } else {
                    "This server cannot transcribe recordings"
                }
                .into(),
                progress: if ready { Some(1.0) } else { None },
            })),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn snapshot(&self) -> SpeechSetupSnapshot {
        self.snapshot
            .lock()
            .expect("speech setup state poisoned")
            .clone()
    }

    #[cfg(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))]
    pub fn start(
        &self,
        service: Arc<margins_workflows::workspace_service::WorkspaceService>,
        principal: margins_workflows::workspace_service::ServicePrincipal,
        jobs: super::remote_asr::RemoteAsrJobs,
    ) -> anyhow::Result<()> {
        if service.asr_available() || self.running.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        *self.snapshot.lock().expect("speech setup state poisoned") = SpeechSetupSnapshot {
            state: "preparing",
            message: "Preparing transcription model".into(),
            progress: None,
        };
        let state = self.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("margins-speech-setup".into())
            .spawn(move || {
                let progress_state = state.clone();
                let result = super::prepare_asr_assets(&|message, progress| {
                    *progress_state
                        .snapshot
                        .lock()
                        .expect("speech setup state poisoned") = SpeechSetupSnapshot {
                        state: "preparing",
                        message,
                        progress,
                    };
                })
                .and_then(|_| {
                    anyhow::ensure!(
                        crate::speech_models::transcription_runtime_available(
                            &crate::settings::load_settings()
                        ),
                        "speech runtime is unavailable after model installation"
                    );
                    Ok(())
                });
                match result {
                    Ok(()) => {
                        service.set_asr_available(true);
                        if let Ok(pending) = service.pending_transcription_jobs() {
                            for job in pending {
                                jobs.schedule(service.clone(), principal.clone(), job);
                            }
                        }
                        *state.snapshot.lock().expect("speech setup state poisoned") =
                            SpeechSetupSnapshot {
                                state: "ready",
                                message: "Transcription ready".into(),
                                progress: Some(1.0),
                            };
                    }
                    Err(error) => {
                        eprintln!("[margins-server] speech setup needs attention: {error:#}");
                        *state.snapshot.lock().expect("speech setup state poisoned") =
                            SpeechSetupSnapshot {
                                state: "failed",
                                message: error.to_string().chars().take(500).collect(),
                                progress: None,
                            };
                    }
                }
                state.running.store(false, Ordering::Release);
            })
        {
            self.running.store(false, Ordering::Release);
            return Err(error.into());
        }
        Ok(())
    }
}
