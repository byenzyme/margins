use async_trait::async_trait;
use chrono::{DateTime, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone};
use margins::session::{self, SessionGrounding};
use pi::sdk::{
    create_agent_session_with_store, default_tool_registry, Config, ContentBlock, SessionOptions,
    SessionStoreKind, TextContent, ThinkingLevel, Tool, ToolFactory, ToolOutput, ToolRegistry,
    ToolUpdate,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::pi_events::{
    assistant_text, debug_raw_trace_file, emit_pi_event, note_text_delta,
    recall_freshness_stale_prefix, recall_result_label,
};
use crate::DISTILL_CANCELLED_SENTINEL;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Serialize, Deserialize)]
pub struct NoteConfig {
    pub inbox_folder: String,
    pub people_folder: String,
    pub created_date_format: String,
    pub note_filename_template: String,
    pub person_note_template: String,
    pub distill_instructions: String,
    pub people: Vec<String>,
    pub people_candidates: Vec<String>,
    pub event_title: Option<String>,
    pub event_start: Option<String>,
}

impl Default for NoteConfig {
    fn default() -> Self {
        Self {
            inbox_folder: "meetings".to_string(),
            people_folder: "people".to_string(),
            created_date_format: "[[%Y-%m-%d]]".to_string(),
            note_filename_template: "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}".to_string(),
            person_note_template: "# {{name}}\n".to_string(),
            distill_instructions: String::new(),
            people: Vec::new(),
            people_candidates: Vec::new(),
            event_title: None,
            event_start: None,
        }
    }
}

pub struct PiDistillRequest {
    pub work_dir: PathBuf,
    pub margins_dir: PathBuf,
    pub trace_dir: PathBuf,
    pub session_name: String,
    pub memo_path: PathBuf,
    pub capture_context: String,
    /// Full both-channel offline aligned timeline (`_aligned.md`), when it was
    /// produced. Richer and more complete than `capture_context`, which may be
    /// assembled from a partial live transcript. Empty when unavailable.
    pub aligned_context: String,
    pub vault_path: Option<PathBuf>,
    pub note_config: NoteConfig,
    pub ai_provider: Option<String>,
    pub ai_model: Option<String>,
    pub ai_api_key: Option<String>,
    pub ai_credential_generation: Option<u64>,
    /// Optional cheaper model used only to plan bounded vault retrieval. The
    /// final note writer still uses `ai_provider`/`ai_model`.
    pub prep_ai_provider: Option<String>,
    pub prep_ai_model: Option<String>,
    pub prep_ai_api_key: Option<String>,
    pub prep_ai_credential_generation: Option<u64>,
    pub skill_path: PathBuf,
    /// Cooperative cancel flag flipped from `cancel_process_session`.
    /// Checked at the await boundaries that bracket the LLM session.
    pub cancel: Arc<AtomicBool>,
    /// When set, resume the persisted Pi session at this path instead of
    /// starting a fresh distillation. Used by the post-completion refine flow:
    /// the full prior conversation (transcript reads, reasoning, the saved note)
    /// is reloaded and the refine message becomes the next turn.
    pub resume_session_path: Option<PathBuf>,
    /// The user's refine instruction. Only honored when `resume_session_path` is
    /// set; replaces the large first-pass distillation prompt.
    pub refine_message: Option<String>,
    /// When set, the streamed final note overwrites exactly this file instead
    /// of deriving a fresh (uniquified) path. Guarantees a refine edits the
    /// note the user is looking at, in place.
    pub existing_note_path: Option<PathBuf>,
    /// First-pass note generation stops at an app-owned draft so the user can
    /// review it before the vault is mutated. Refine/save paths set this true.
    pub save_generated_note: bool,
}

/// What a distill/refine run produced. `session_file` is the persisted Pi
/// session path, captured so a later refine can resume the same conversation.
pub struct PiDistillOutcome {
    pub note_path: Option<String>,
    pub session_file: Option<String>,
}

type Emit = Arc<dyn Fn(&str, &str, Option<f32>) + Send + Sync>;

pub fn run_pi_distill_blocking(
    request: PiDistillRequest,
    emit: impl Fn(&str, &str, Option<f32>) + Send + Sync + 'static,
) -> Result<PiDistillOutcome, String> {
    let reactor = asupersync::runtime::reactor::create_reactor()
        .map_err(|e| format!("failed to create AI runtime: {e}"))?;
    let runtime = asupersync::runtime::RuntimeBuilder::current_thread()
        .with_reactor(reactor)
        .build()
        .map_err(|e| format!("failed to build AI runtime: {e}"))?;
    runtime.block_on(async move { run_pi_distill_async(request, Arc::new(emit)).await })
}

async fn run_pi_distill_async(
    request: PiDistillRequest,
    emit: Emit,
) -> Result<PiDistillOutcome, String> {
    let cancel = request.cancel.clone();
    fn bail_if_cancelled(cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::SeqCst) {
            Err(DISTILL_CANCELLED_SENTINEL.to_string())
        } else {
            Ok(())
        }
    }
    bail_if_cancelled(&cancel)?;
    // Best-effort: proactively refresh any expired openai-codex OAuth token so
    // the model handoff below never fails on a silently-refreshable token.
    // No network call unless a token is within pi's proactive window; all
    // errors are swallowed so distill is never aborted by a refresh failure.
    #[cfg(feature = "tauri-app")]
    crate::ai_auth::refresh_pi_tokens().await;
    let skill_path = request.skill_path.clone();
    if !skill_path.exists() {
        return Err(format!(
            "Cannot find bundled Margins Desktop skill at {}",
            skill_path.display()
        ));
    }
    let desktop_host = std::fs::read_to_string(&skill_path).map_err(|e| {
        format!(
            "failed to read margins desktop host preamble {}: {e}",
            skill_path.display()
        )
    })?;
    let core_path = resolve_distillation_core_path(&skill_path)?;
    let distillation_core = std::fs::read_to_string(&core_path).map_err(|e| {
        format!(
            "failed to read shared margins distillation core {}: {e}",
            core_path.display()
        )
    })?;
    let memo = std::fs::read_to_string(&request.memo_path).unwrap_or_default();
    let note_config = note_config_with_source_frontmatter(request.note_config.clone(), &memo);
    let (bundle_template_dir, override_template_dir) =
        resolve_template_dirs(&request.margins_dir, &skill_path)?;
    let template_bundle =
        load_margins_template_bundle(&bundle_template_dir, override_template_dir.as_deref());

    let tool_factory = Arc::new(MarginsPiToolFactory {
        vault_path: request.vault_path.clone(),
    });

    (emit)(
        "synthesize",
        "Starting AI note distillation using your configured model...",
        Some(0.62),
    );

    let _api_key_env = ScopedEnvVar::set("MARGINS_API_KEY", request.ai_api_key.as_deref());
    let tool_work_dir = tool_work_dir_for_request(&request);
    let final_enabled_tools = if request.resume_session_path.is_some() {
        // Legacy refine may resume an older tool-using conversation; keep the
        // existing tool set available for compatibility.
        Some(vec![
            "grep".into(),
            "ls".into(),
            "enzyme_petri".into(),
            "enzyme_catalyze".into(),
        ])
    } else {
        // First-pass distill now receives a compact precomputed vault context
        // bundle. Keep the strong final writer out of exploratory tool loops.
        Some(vec![])
    };
    let options = SessionOptions {
        provider: request.ai_provider.clone(),
        model: request.ai_model.clone(),
        api_key: request.ai_api_key.clone(),
        working_directory: Some(tool_work_dir.clone()),
        no_session: false,
        session_path: request.resume_session_path.clone(),
        session_dir: Some(request.trace_dir.join("pi-sessions")),
        enabled_tools: final_enabled_tools,
        thinking: Some(ThinkingLevel::Medium),
        max_tool_iterations: 30,
        tool_factory: Some(tool_factory),
        append_system_prompt: Some(
            "You are running inside Margins Desktop. Preserve user data and do not delete artifacts. For the final answer, stream only the complete Markdown note as normal assistant text; Margins Desktop will persist it.".to_string(),
        ),
        ..SessionOptions::default()
    };

    bail_if_cancelled(&cancel)?;

    let mut session = create_agent_session_with_store(options, SessionStoreKind::Jsonl)
        .await.map_err(|e| {
        let disposition = handle_pi_provider_error(
            &request,
            PiCredentialLane::Final,
            "distill_setup",
        );
        if disposition == crate::ai_config::ProviderErrorDisposition::Cancelled {
            DISTILL_CANCELLED_SENTINEL.to_string()
        } else {
            format!("AI model setup failed. Sign in with ChatGPT or check your API key/base URL in Settings. Details: {e}")
        }
    })?;
    if let Ok(raw) = std::env::var("MARGINS_UX_E2E_MAX_TOKENS") {
        if let Ok(max_tokens) = raw.trim().parse::<u32>() {
            if max_tokens > 0 {
                session.set_max_tokens(Some(max_tokens));
            }
        }
    }

    bail_if_cancelled(&cancel)?;
    let (provider, model) = session.model();
    (emit)(
        "synthesize",
        &format!("AI model: {provider}/{model}"),
        Some(0.66),
    );

    let notes_folder_text = request
        .vault_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no notes folder configured; create a local note)".to_string());

    let vault_context_bundle = if request.resume_session_path.is_none() {
        prepare_vault_context_bundle(&request, &emit, &cancel)
            .await
            .unwrap_or_else(|error| format!("Vault context prep unavailable: {error}"))
    } else {
        String::new()
    };

    // Prefer the full both-channel aligned timeline as the primary source when
    // it was produced. The capture context can be lopsided (single channel or
    // only the tail of the conversation) because it is assembled from the live
    // transcript; the aligned timeline is the complete offline record.
    let has_aligned = !request.aligned_context.trim().is_empty();
    let primary_source_line = if has_aligned {
        "- Treat the full aligned timeline (`# Aligned timeline` below) as the primary source of truth for what was said. It is the complete offline both-channel transcript. The `# Capture context` section is a lighter live sidecar that may be partial (single channel or only the tail); use it only to corroborate, never in place of the aligned timeline when the two diverge."
    } else {
        "- Treat the supplied capture context as the source of truth for transcript+memo context."
    };
    let aligned_section = if has_aligned {
        format!("\n# Aligned timeline\n\n{}\n", request.aligned_context)
    } else {
        String::new()
    };
    let aligned_path_line = if has_aligned {
        format!(
            "Aligned timeline path: {}\n",
            request
                .margins_dir
                .join(format!("{}_aligned.md", request.session_name))
                .display()
        )
    } else {
        String::new()
    };
    let participant_guidance = participant_guidance_text(&note_config);

    let prompt = format!(
        r#"Run the Margins Desktop distillation skill for this capture.

Margins Desktop has supplied the memo, capture context, and full transcript artifact below. Produce the connected note from that prepared context, the templates, note configuration, and vault context.

Shared distillation rules:
- Apply `# Margins distillation core` as the single source of truth for interpretation, people handling, template choice, vault evidence, frontmatter matching, grounding markers, and writing style.
- Apply `# Margins Desktop host preamble` for desktop constraints: use only supplied data blocks, do not rediscover artifacts, and output only final Markdown.
- Treat the data blocks below as authoritative. {primary_source_line}
- Write from the local user's perspective: the mic/local channel is the user and should be rendered as first person (`I`, `me`, `my`) when using the 1:1 template.
- Use configured people metadata to name the other participant(s) when evidence supports it. {participant_guidance}
- Use the precomputed `# Vault context bundle` below as the complete vault evidence for this run. Do not run more vault search from the final writer.
- Make the title the first visible content after YAML frontmatter. It must be a 3-7 word natural-language title that names the central tension or specific object, never a generic recap label. Immediately after it, write one concise user-facing orientation paragraph before the first hidden grounding marker.
- Output raw Markdown only. Do not wrap the note or the YAML frontmatter in a code fence; the response must start directly with `---`.
- Do not add a visible `Grounding`, `Sources`, or memo-accounting section.
- Final action: output the complete final Markdown note as normal assistant text. Do not call a save-note tool; Margins Desktop streams, displays, and persists your Markdown.

Configured note instructions:
{artifact_instructions}

Session: {session}
Tool working directory: {tool_work_dir}
Session workspace: {work_dir}
Margins metadata directory: {margins_dir}
Notes folder: {notes_folder}
Memo path: {memo_path}
Capture context path: {capture_context_path}
{aligned_path_line}Bundled template directory (source of truth): {template_dir}

# Margins Desktop host preamble

{desktop_host}

# Margins distillation core

{distillation_core}

# Margins note templates

These are the concrete template files referenced by the skill. They are loaded from the bundled defaults shipped with the skill, unless a matching file exists in the session's `.margins/templates/` directory, which is honored as an explicit user override. Select one explicitly before drafting; default to `1on1-idea-exchange.md` for ordinary 1:1 idea exchanges, catch-ups, brainstorms, and relationship-building conversations.

{template_bundle}

# Memo

{memo}

# Capture context

{capture_context}
{aligned_section}
# Vault context bundle

{vault_context_bundle}"#,
        artifact_instructions = artifact_instruction_text(&note_config),
        session = request.session_name,
        tool_work_dir = tool_work_dir.display(),
        work_dir = request.work_dir.display(),
        margins_dir = request.margins_dir.display(),
        notes_folder = notes_folder_text,
        memo_path = request.memo_path.display(),
        capture_context_path = request
            .margins_dir
            .join(format!("{}_capture_context.md", request.session_name))
            .display(),
        template_dir = bundle_template_dir.display(),
        desktop_host = desktop_host,
        distillation_core = distillation_core,
        template_bundle = template_bundle,
        capture_context = request.capture_context,
        primary_source_line = primary_source_line,
        participant_guidance = participant_guidance,
        aligned_path_line = aligned_path_line,
        aligned_section = aligned_section,
        vault_context_bundle = vault_context_bundle,
    );

    // Refine turn: resume the persisted conversation and replace the heavy
    // first-pass prompt with the user's instruction. The agent already holds the
    // transcript, templates, prior reasoning, and the note it wrote.
    let prompt = if let Some(message) = request.refine_message.clone() {
        let target = request
            .existing_note_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "the note you already saved".to_string());
        format!(
            r#"The user has reviewed the connected note you saved and is asking for a change. Apply it and re-save.

User request:
{message}

Requirements:
- Treat this as a revision of the existing note, not a new note. Keep everything the user did not ask to change.
- Keep using the aligned timeline / capture context already in this conversation as the source of truth. Do not invent facts to satisfy the request; if the request cannot be grounded, say so briefly and make the closest supported change.
- Preserve the YAML frontmatter key set, the forward-looking open questions / bring-forward section, the consolidated `### Action items` section, wikilinks, and the hidden `<!--MARGINS:USE {{json}}-->` grounding markers (update markers that change, including `note_quote` anchors).
- Final action: output the full revised Markdown as normal assistant text. Margins Desktop will overwrite the saved note at {target} in place. Do not create a second note."#,
        )
    } else {
        prompt
    };

    let draft = Arc::new(Mutex::new(StreamingNoteDraft::new(
        &request.margins_dir,
        &request.session_name,
    )));
    let emit_for_events = emit.clone();
    let tool_labels = Arc::new(Mutex::new(HashMap::new()));
    let tool_labels_for_events = tool_labels.clone();
    let raw_trace = debug_raw_trace_file(&request.trace_dir, &request.session_name);
    let raw_trace_for_events = raw_trace.clone();
    let cancel_for_events = cancel.clone();
    let draft_for_events = draft.clone();
    let assistant = session
        .prompt(prompt, move |event| {
            if cancel_for_events.load(Ordering::SeqCst) {
                return;
            }
            if let Some(delta) = note_text_delta(&event) {
                if let Ok(mut draft) = draft_for_events.lock() {
                    draft.push(&delta);
                }
            }
            emit_pi_event(
                emit_for_events.as_ref(),
                &tool_labels_for_events,
                &raw_trace_for_events,
                event,
            )
        })
        .await
        .map_err(|e| {
            let disposition =
                handle_pi_provider_error(&request, PiCredentialLane::Final, "distill_prompt");
            if disposition == crate::ai_config::ProviderErrorDisposition::Cancelled {
                DISTILL_CANCELLED_SENTINEL.to_string()
            } else {
                format!("AI note distillation failed: {e}")
            }
        })?;

    bail_if_cancelled(&cancel)?;

    // Capture the persisted session file so a later refine can resume this exact
    // conversation. The prompt has returned, so the session is idle and a
    // non-blocking try_lock succeeds.
    let session_file = session
        .into_inner()
        .session
        .try_lock()
        .ok()
        .and_then(|guard| guard.path.clone())
        .map(|p| p.to_string_lossy().to_string());

    let streamed_raw = draft
        .lock()
        .ok()
        .map(|draft| draft.buffer().trim().to_string())
        .unwrap_or_default();
    let note_raw = if streamed_raw.is_empty() {
        assistant_text(&assistant.content).trim().to_string()
    } else {
        streamed_raw
    };
    let (note_markdown, marker_grounding) = strip_margins_markers(&note_raw);
    let note_markdown = note_markdown.trim().to_string();
    if note_markdown.is_empty() {
        (emit)(
            "synthesize",
            "AI note run completed without a saved note artifact",
            Some(0.9),
        );
        return Ok(PiDistillOutcome {
            note_path: None,
            session_file,
        });
    }

    if !request.save_generated_note {
        bail_if_cancelled(&cancel)?;
        let draft_path = draft
            .lock()
            .ok()
            .map(|draft| draft.path.clone())
            .unwrap_or_else(|| {
                request
                    .margins_dir
                    .join(format!("{}_note_draft.md", request.session_name))
            });
        if let Some(parent) = draft_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to prepare AI note draft: {e}"))?;
        }
        let draft_markdown =
            ensure_note_frontmatter(&note_markdown, &note_config, &request.session_name);
        std::fs::write(&draft_path, draft_markdown)
            .map_err(|e| format!("failed to save AI note draft: {e}"))?;
        if let Some(grounding) = marker_grounding {
            let _ = session::set_session_grounding(
                &request.margins_dir,
                &request.session_name,
                &grounding,
            );
        }
        (emit)("synthesize", "Draft note ready for review", Some(0.9));
        return Ok(PiDistillOutcome {
            note_path: None,
            session_file,
        });
    }

    let note_path = save_markdown_note(SaveMarkdownRequest {
        vault_path: request.vault_path.as_deref(),
        margins_dir: &request.margins_dir,
        session_name: &request.session_name,
        note_config: &note_config,
        existing_note_path: request.existing_note_path.as_deref(),
        markdown: &note_markdown,
        grounding: marker_grounding,
        cancel: Some(&cancel),
    })
    .map_err(|e| format!("failed to save streamed AI note: {e}"))?;
    remove_note_draft(&request.margins_dir, &request.session_name);
    (emit)(
        "synthesize",
        &format!("Saved note to {}", note_path.display()),
        Some(0.9),
    );
    Ok(PiDistillOutcome {
        note_path: Some(note_path.to_string_lossy().to_string()),
        session_file,
    })
}

struct StreamingNoteDraft {
    path: PathBuf,
    buffer: String,
    file: Option<std::fs::File>,
}

impl StreamingNoteDraft {
    fn new(margins_dir: &Path, session_name: &str) -> Self {
        let path = margins_dir.join(format!("{session_name}_note_draft.md"));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = std::fs::File::create(&path).ok();
        Self {
            path,
            buffer: String::new(),
            file,
        }
    }

    fn push(&mut self, delta: &str) {
        self.buffer.push_str(delta);
        if let Some(file) = self.file.as_mut() {
            let _ = file.write_all(delta.as_bytes());
        }
    }

    fn buffer(&self) -> &str {
        &self.buffer
    }
}

pub(crate) fn remove_note_draft(margins_dir: &Path, session_name: &str) {
    let _ = std::fs::remove_file(margins_dir.join(format!("{session_name}_note_draft.md")));
}

pub(crate) struct SaveMarkdownRequest<'a> {
    pub(crate) vault_path: Option<&'a Path>,
    pub(crate) margins_dir: &'a Path,
    pub(crate) session_name: &'a str,
    pub(crate) note_config: &'a NoteConfig,
    pub(crate) existing_note_path: Option<&'a Path>,
    pub(crate) markdown: &'a str,
    pub(crate) grounding: Option<Vec<SessionGrounding>>,
    pub(crate) cancel: Option<&'a AtomicBool>,
}

pub(crate) fn save_markdown_note(request: SaveMarkdownRequest<'_>) -> std::io::Result<PathBuf> {
    let markdown =
        ensure_note_frontmatter(request.markdown, request.note_config, request.session_name);

    let path = if let Some(existing) = request.existing_note_path {
        existing.to_path_buf()
    } else {
        let filename = note_filename_from_config(
            request.note_config,
            request.session_name,
            note_title_for_filename(&markdown).as_deref(),
        );
        let requested_path = if let Some(vault) = request.vault_path {
            vault
                .join(request.note_config.inbox_folder.trim())
                .join(filename)
        } else {
            request
                .margins_dir
                .join(format!("{}_note.md", request.session_name))
        };
        unique_available_note_path(&requested_path)
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Some(vault) = request.vault_path {
        let final_people = crate::note_artifacts::parse_note_frontmatter(&markdown).people;
        let mut people_note_config = request.note_config.clone();
        people_note_config.people = merge_people(&request.note_config.people, &final_people);
        ensure_people_files(vault, &people_note_config).map_err(std::io::Error::other)?;
    }
    if request
        .cancel
        .map(|cancel| cancel.load(Ordering::SeqCst))
        .unwrap_or(false)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ));
    }
    let previous = std::fs::read(&path).ok();
    std::fs::write(&path, markdown)?;
    if request
        .cancel
        .map(|cancel| cancel.load(Ordering::SeqCst))
        .unwrap_or(false)
    {
        if let Some(previous) = previous {
            let _ = std::fs::write(&path, previous);
        } else {
            let _ = std::fs::remove_file(&path);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ));
    }
    if let Some(grounding) = request.grounding {
        let _ =
            session::set_session_grounding(request.margins_dir, request.session_name, &grounding);
    }
    let _ = std::fs::remove_file(
        request
            .margins_dir
            .join(format!("{}_grounding.json", request.session_name)),
    );

    Ok(path)
}

struct ScopedEnvVar {
    key: &'static str,
    previous: Option<String>,
    active: bool,
}

impl ScopedEnvVar {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let previous = std::env::var(key).ok();
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Self {
                key,
                previous,
                active: false,
            };
        };
        std::env::set_var(key, value);
        Self {
            key,
            previous,
            active: true,
        }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Some(previous) = &self.previous {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn artifact_instruction_text(config: &NoteConfig) -> String {
    format!(
        "- Meeting notes folder: {inbox}\n- People folder: {people_folder}\n- Created date format: {created_fmt}\n- Filename template: {filename}\n- Calendar event title: {event}\n- Calendar event start: {start}\n- People/attendees: {people}\n- Recent people candidates: {people_candidates}\n\n{instructions}",
        inbox = config.inbox_folder,
        people_folder = config.people_folder,
        created_fmt = config.created_date_format,
        filename = config.note_filename_template,
        event = config
            .event_title
            .clone()
            .unwrap_or_else(|| "(none)".to_string()),
        start = config
            .event_start
            .clone()
            .unwrap_or_else(|| "(none)".to_string()),
        people = if config.people.is_empty() {
            "(none)".to_string()
        } else {
            config.people.join(", ")
        },
        people_candidates = if config.people_candidates.is_empty() {
            "(none)".to_string()
        } else {
            config
                .people_candidates
                .iter()
                .take(50)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        },
        instructions = config.distill_instructions,
    )
}

fn participant_guidance_text(config: &NoteConfig) -> String {
    let people = clean_people(&config.people);
    match people.as_slice() {
        [] => "If no other participant is configured, infer names only from transcript evidence."
            .to_string(),
        [person] => format!(
            "For an ordinary 1:1 capture, treat {person} as the other person and prefer headings like `What {person} thinks` over the placeholder `What [other person] thinks`."
        ),
        people => format!(
            "Configured attendees: {}. Keep the local user's channel in first person, and attribute remote/other-person claims to the named attendee when the transcript supports it.",
            people.join(", ")
        ),
    }
}

fn tool_work_dir_for_request(request: &PiDistillRequest) -> PathBuf {
    request
        .vault_path
        .as_ref()
        .and_then(|vault| common_ancestor(&request.work_dir, vault))
        .unwrap_or_else(|| request.work_dir.clone())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DistillPrepPlan {
    petri_query: String,
    catalyze_queries: Vec<String>,
}

async fn prepare_vault_context_bundle(
    request: &PiDistillRequest,
    emit: &Emit,
    cancel: &AtomicBool,
) -> Result<String, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }
    let Some(vault) = request.vault_path.as_ref() else {
        return Ok("No notes folder configured; write from transcript and memo only.".to_string());
    };

    emit(
        "synthesize",
        "Preparing compact vault context with bounded recall searches...",
        Some(0.64),
    );
    let memo = std::fs::read_to_string(&request.memo_path).unwrap_or_default();
    let plan = plan_vault_context(request, &memo).await;
    let plan = match plan {
        Ok(plan) => plan,
        Err(error) => {
            emit(
                "synthesize",
                &format!(
                    "Cheap context planner unavailable; using deterministic query plan. {error}"
                ),
                None,
            );
            deterministic_prep_plan_for_request(request, &memo)
        }
    };

    let petri_query = clean_query(&plan.petri_query)
        .unwrap_or_else(|| deterministic_prep_plan_for_request(request, &memo).petri_query);
    let catalyze_queries = normalize_catalyze_queries(plan.catalyze_queries, &petri_query, request);
    let catalyze_limit = catalyze_query_limit(request, catalyze_queries.len());

    if cancel.load(Ordering::SeqCst) {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }

    let selected_queries = catalyze_queries
        .iter()
        .take(catalyze_limit)
        .cloned()
        .collect::<Vec<_>>();
    // Petri and Catalyze are independent, read-only queries over the same
    // immutable vault index. Launch the whole bounded retrieval batch together
    // so Petri startup/runtime no longer sits serially ahead of Catalyze.
    let (petri, catalyze_runs) = std::thread::scope(|scope| {
        let petri_vault = vault.clone();
        let petri_query_for_worker = petri_query.clone();
        let petri_handle = scope.spawn(move || {
            run_enzyme(
                vec![
                    "petri".to_string(),
                    "--vault".to_string(),
                    petri_vault.to_string_lossy().to_string(),
                    "--top".to_string(),
                    "8".to_string(),
                    "--catalyst-budget".to_string(),
                    "2".to_string(),
                    "--query".to_string(),
                    petri_query_for_worker,
                ],
                Some(petri_vault.as_path()),
            )
        });
        let catalyze_handles = selected_queries
            .iter()
            .cloned()
            .map(|query| {
                let vault = vault.clone();
                scope.spawn(move || {
                    let result = run_enzyme(
                        vec![
                            "catalyze".to_string(),
                            "--vault".to_string(),
                            vault.to_string_lossy().to_string(),
                            "--limit".to_string(),
                            "5".to_string(),
                            query.clone(),
                        ],
                        Some(vault.as_path()),
                    );
                    (query, result)
                })
            })
            .collect::<Vec<_>>();
        let petri = petri_handle
            .join()
            .map_err(|_| "The bounded vault-mapping worker panicked.".to_string())?;
        let catalyze = catalyze_handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| "A bounded note-snippet worker panicked.".to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok::<_, String>((petri, catalyze))
    })?;
    let petri_summary = match &petri {
        Ok(run) => summarize_petri_stdout(&run.stdout),
        Err(run) => format!(
            "Vault-map query failed: {}",
            truncate_for_details(&format!("{} {}", run.stderr, run.stdout), 800)
        ),
    };
    if let Ok(run) = &petri {
        emit(
            "synthesize",
            &format!(
                "Mapped related evidence ideas from your vault ({} chars).",
                run.stdout.len()
            ),
            None,
        );
    }

    if cancel.load(Ordering::SeqCst) {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }
    let mut catalyze_sections = Vec::new();
    for (query, result) in catalyze_runs {
        match &result {
            Ok(run) => {
                emit(
                    "synthesize",
                    &format!(
                        "Found related evidence snippets for '{}' ({} ms).",
                        truncate_for_details(&query, 70),
                        run.duration_ms,
                    ),
                    None,
                );
                catalyze_sections.push(format!(
                    "## Related-evidence query: {}\n\n{}",
                    query,
                    summarize_catalyze_stdout(&run.stdout)
                ));
            }
            Err(run) => catalyze_sections.push(format!(
                "## Related-evidence query: {}\n\nSearch failed: {}",
                query,
                truncate_for_details(&format!("{} {}", run.stderr, run.stdout), 800)
            )),
        }
    }

    let petri_chars = petri
        .as_ref()
        .map(|run| run.stdout.len())
        .unwrap_or_default();
    let bundle = format!(
        "Prepared by Margins before final synthesis. No grep was used.\n\nUse this bundle to enrich the note, not merely to decorate it. For the strongest related snippets, carry the prior context into the relevant section as a concrete bridge: prior evidence -> recurring theme/constraint/open thread -> why it changes how this capture should be read. Prefer a few rich bridges over many shallow citations.\n\nVault-map query: {petri_query}\nVault-map payload chars before compaction: {petri_chars}\nRelated-evidence queries:\n{queries}\n\n# Ranked Vault Summary\n\n{petri_summary}\n\n# Related Evidence Snippets\n\n{catalyze}",
        queries = catalyze_queries
            .iter()
            .take(catalyze_limit)
            .map(|q| format!("- {q}"))
            .collect::<Vec<_>>()
            .join("\n"),
        catalyze = catalyze_sections.join("\n\n")
    );
    Ok(truncate_for_details(&bundle, 28000))
}

async fn plan_vault_context(
    request: &PiDistillRequest,
    memo: &str,
) -> Result<DistillPrepPlan, String> {
    let note_config = note_config_with_source_frontmatter(request.note_config.clone(), memo);
    if !should_use_prep_model(request) {
        return Ok(deterministic_prep_plan_for_request(request, memo));
    }

    let _prep_api_key_env =
        ScopedEnvVar::set("MARGINS_API_KEY", request.prep_ai_api_key.as_deref());
    let options = SessionOptions {
        provider: request.prep_ai_provider.clone(),
        model: request.prep_ai_model.clone(),
        api_key: request.prep_ai_api_key.clone(),
        working_directory: Some(tool_work_dir_for_request(request)),
        no_session: true,
        enabled_tools: Some(vec![]),
        thinking: Some(ThinkingLevel::Low),
        max_tool_iterations: 0,
        append_system_prompt: Some(
            "You plan bounded note-context retrieval for Margins Desktop. Output only JSON."
                .to_string(),
        ),
        ..SessionOptions::default()
    };
    let mut session = create_agent_session_with_store(options, SessionStoreKind::Jsonl)
        .await
        .map_err(|e| {
            let disposition =
                handle_pi_provider_error(request, PiCredentialLane::Prep, "prep_setup");
            if disposition == crate::ai_config::ProviderErrorDisposition::Cancelled {
                DISTILL_CANCELLED_SENTINEL.to_string()
            } else {
                format!("prep model setup failed: {e}")
            }
        })?;
    session.set_max_tokens(Some(300));

    let prompt = format!(
        r#"Create a compact related-note retrieval plan for this capture.

Return only JSON with this exact shape:
{{"petri_query":"...","catalyze_queries":["...","...","..."]}}

Rules:
- The primary query should be one sentence or keyword phrase from the memo/transcript.
- Include 2 to 4 related-note queries. Use 4 when named people, projects, or community names point to distinct retrieval angles.
- Use the vault's own vocabulary from known context when available; avoid generic category labels.
- Follow language that feels alive, distinctive, or still forming — memo-marked lines, surprising turns, charged phrasing — not only the obvious transcript topic.
- When the memo contains a distinctive or user-marked phrase, let it shape one of the related-note queries.
- Also draw on people, projects, tools, tensions, and concrete anchors when they fit; do not force a vault connection when nothing resonates.
- Include configured people/attendees and calendar title when they disambiguate the meeting, but do not make every query just a person name.
- Include recent people candidates when they help disambiguate a likely misspelling or transcript name.
- Do not include grep or filesystem steps.

# Configured people/attendees
{people}

# Recent people candidates
{people_candidates}

# Calendar/event
{event}

# Memo
{memo}

# Transcript excerpt
{transcript}"#,
        people = people_list_text(&note_config.people),
        people_candidates = people_list_text(
            &note_config
                .people_candidates
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
        ),
        event = note_config.event_title.as_deref().unwrap_or("(none)"),
        memo = truncate_for_details(memo, 1200),
        transcript = transcript_excerpt(&request.aligned_context, &request.capture_context, 3500)
    );
    let assistant = session.prompt(prompt, |_| {}).await.map_err(|e| {
        let disposition = handle_pi_provider_error(request, PiCredentialLane::Prep, "prep_prompt");
        if disposition == crate::ai_config::ProviderErrorDisposition::Cancelled {
            DISTILL_CANCELLED_SENTINEL.to_string()
        } else {
            format!("prep model plan failed: {e}")
        }
    })?;
    let text = assistant_text(&assistant.content);
    parse_prep_plan(&text).ok_or_else(|| "prep model did not return valid plan JSON".to_string())
}

#[derive(Clone, Copy)]
enum PiCredentialLane {
    Final,
    Prep,
}

fn handle_pi_provider_error(
    request: &PiDistillRequest,
    lane: PiCredentialLane,
    stage: &'static str,
) -> crate::ai_config::ProviderErrorDisposition {
    let generation = match lane {
        PiCredentialLane::Final => request.ai_credential_generation,
        PiCredentialLane::Prep => request.prep_ai_credential_generation,
    };
    crate::ai_config::handle_included_ai_provider_error(
        generation,
        Some(request.cancel.as_ref()),
        stage,
    )
}

fn should_use_prep_model(request: &PiDistillRequest) -> bool {
    let Some(provider) = request.prep_ai_provider.as_deref() else {
        return false;
    };
    let Some(model) = request.prep_ai_model.as_deref() else {
        return false;
    };
    if provider.trim().is_empty() || model.trim().is_empty() {
        return false;
    }
    request.prep_ai_provider != request.ai_provider
        || request.prep_ai_model != request.ai_model
        || request.prep_ai_api_key != request.ai_api_key
}

fn deterministic_prep_plan_for_request(request: &PiDistillRequest, memo: &str) -> DistillPrepPlan {
    let note_config = note_config_with_source_frontmatter(request.note_config.clone(), memo);
    let people = clean_people(&note_config.people);
    let event_text = note_config.event_title.as_deref().unwrap_or_default();
    let aligned_seed = request
        .aligned_context
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("what")
                || lower.contains("trust")
                || lower.contains("agent")
                || lower.contains("research")
                || lower.contains("project")
                || people
                    .iter()
                    .any(|person| lower.contains(&person.to_ascii_lowercase()))
        })
        .take(12)
        .collect::<Vec<_>>()
        .join(" ");
    let plan = deterministic_prep_plan(
        &format!("{} {} {}", people.join(" "), event_text, memo),
        &aligned_seed,
    );
    let mut queries = Vec::new();
    if let Some(event) = note_config.event_title.as_deref().and_then(clean_query) {
        queries.push(event);
    }
    if !people.is_empty() {
        queries.push(format!("{} {}", people.join(" "), plan.petri_query));
    }
    queries.extend(plan.catalyze_queries);
    DistillPrepPlan {
        petri_query: plan.petri_query,
        catalyze_queries: queries,
    }
}

fn deterministic_prep_plan(memo: &str, aligned_context: &str) -> DistillPrepPlan {
    let seed = clean_query(&format!(
        "{} {}",
        memo.lines().take(8).collect::<Vec<_>>().join(" "),
        aligned_context
            .lines()
            .filter(|line| {
                let lower = line.to_ascii_lowercase();
                lower.contains("what") || lower.contains("trust") || lower.contains("agent")
            })
            .take(12)
            .collect::<Vec<_>>()
            .join(" ")
    ))
    .unwrap_or_else(|| "meeting themes decisions tensions action items".to_string());
    let short = truncate_for_details(&seed, 220);
    DistillPrepPlan {
        petri_query: short.clone(),
        catalyze_queries: vec![
            short.clone(),
            format!("{short} related prior notes people projects"),
            format!("{short} decisions tensions action items"),
        ],
    }
}

fn normalize_catalyze_queries(
    mut queries: Vec<String>,
    fallback: &str,
    request: &PiDistillRequest,
) -> Vec<String> {
    let event_query = request
        .note_config
        .event_title
        .as_deref()
        .and_then(clean_query);
    if let Some(event) = event_query.as_deref() {
        queries.push(event.to_string());
    }
    if !request.note_config.people.is_empty() {
        queries.push(format!(
            "{} {}",
            request.note_config.people.join(" "),
            fallback
        ));
    }
    for person in request.note_config.people.iter().take(3) {
        queries.push(format!("{person} {fallback}"));
        if let Some(event) = event_query.as_deref() {
            queries.push(format!("{person} {event}"));
        }
    }
    queries = normalize_raw_catalyze_queries(queries)
        .into_iter()
        .take(4)
        .collect();
    if queries.is_empty() {
        queries.push(fallback.to_string());
    }
    while queries.len() < 2 {
        queries.push(format!("{} related notes", fallback));
    }
    queries
}

fn normalize_raw_catalyze_queries(queries: Vec<String>) -> Vec<String> {
    queries
        .into_iter()
        .filter_map(|query| clean_query(&query))
        .fold(Vec::<String>::new(), |mut unique, query| {
            if !unique
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&query))
            {
                unique.push(query);
            }
            unique
        })
}

fn catalyze_query_limit(request: &PiDistillRequest, available: usize) -> usize {
    let richer_context = !request.note_config.people.is_empty()
        || request
            .note_config
            .event_title
            .as_deref()
            .is_some_and(|title| !title.trim().is_empty());
    let target = if richer_context { 4 } else { 3 };
    available.min(target).max(2)
}

fn clean_query(query: &str) -> Option<String> {
    let cleaned = query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    if cleaned.len() < 8 {
        None
    } else {
        Some(truncate_for_details(&cleaned, 260))
    }
}

fn clean_people(people: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for person in people {
        let cleaned = person
            .trim()
            .trim_start_matches("[[")
            .trim_end_matches("]]")
            .trim()
            .to_string();
        if !cleaned.is_empty() && !out.iter().any(|existing| existing == &cleaned) {
            out.push(cleaned);
        }
    }
    out
}

fn people_list_text(people: &[String]) -> String {
    let people = clean_people(people);
    if people.is_empty() {
        "(none)".to_string()
    } else {
        people.join(", ")
    }
}

fn transcript_excerpt(aligned: &str, capture: &str, max_chars: usize) -> String {
    let source = if aligned.trim().is_empty() {
        capture
    } else {
        aligned
    };
    if source.chars().count() <= max_chars {
        return source.to_string();
    }
    let head = max_chars / 2;
    let tail = max_chars - head;
    format!(
        "{}\n\n[...middle omitted for prep planning...]\n\n{}",
        truncate_for_details(source, head),
        source
            .chars()
            .rev()
            .take(tail)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>()
    )
}

fn parse_prep_plan(text: &str) -> Option<DistillPrepPlan> {
    serde_json::from_str::<DistillPrepPlan>(text.trim())
        .ok()
        .or_else(|| {
            let start = text.find('{')?;
            let end = text.rfind('}')?;
            serde_json::from_str::<DistillPrepPlan>(&text[start..=end]).ok()
        })
        .and_then(|mut plan| {
            plan.petri_query = clean_query(&plan.petri_query)?;
            plan.catalyze_queries = normalize_raw_catalyze_queries(plan.catalyze_queries)
                .into_iter()
                .take(4)
                .collect();
            Some(plan)
        })
}

fn summarize_petri_stdout(stdout: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return truncate_for_details(stdout, 5000);
    };
    let mut out = String::new();
    if let Some(entities) = value.get("entities").and_then(Value::as_array) {
        for entity in entities.iter().take(8) {
            let name = entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unnamed)");
            let entity_type = entity.get("type").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!("- {name} {entity_type}\n"));
            if let Some(catalysts) = entity.get("catalysts").and_then(Value::as_array) {
                for catalyst in catalysts.iter().take(2) {
                    if let Some(text) = catalyst.get("text").and_then(Value::as_str) {
                        out.push_str(&format!("  - {}\n", truncate_for_details(text, 220)));
                    }
                }
            }
        }
    }
    if out.trim().is_empty() {
        truncate_for_details(stdout, 5000)
    } else {
        out
    }
}

fn summarize_catalyze_stdout(stdout: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        return truncate_for_details(stdout, 8000);
    };
    let mut out = recall_freshness_stale_prefix(&value);
    if let Some(results) = value.get("results").and_then(Value::as_array) {
        for result in results.iter().take(5) {
            let label = recall_result_label(result);
            let similarity = result
                .get("similarity")
                .and_then(Value::as_f64)
                .map(|n| format!("{n:.3}"))
                .unwrap_or_else(|| "n/a".to_string());
            let content = result.get("content").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!(
                "### {label}\nSimilarity: {similarity}\n{}\n\n",
                truncate_for_details(content, 1300)
            ));
        }
    }
    if let Some(catalysts) = value
        .get("top_contributing_catalysts")
        .and_then(Value::as_array)
    {
        let lines = catalysts
            .iter()
            .take(4)
            .filter_map(|c| {
                let entity = c.get("entity").and_then(Value::as_str).unwrap_or("");
                let text = c.get("text").and_then(Value::as_str)?;
                Some(format!("- {entity}: {}", truncate_for_details(text, 180)))
            })
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            out.push_str("Top catalysts:\n");
            out.push_str(&lines.join("\n"));
            out.push('\n');
        }
    }
    if out.trim().is_empty() {
        truncate_for_details(stdout, 8000)
    } else {
        out
    }
}

fn note_config_with_source_frontmatter(mut config: NoteConfig, memo: &str) -> NoteConfig {
    let source_people = crate::note_artifacts::parse_note_frontmatter(memo).people;
    config.people = merge_people(&config.people, &source_people);
    config
}

const MARGINS_TEMPLATE_SPECS: &[(&str, &str)] = &[
    (
        "1on1-idea-exchange.md",
        "Default for most 1:1 conversations, idea exchanges, catch-ups, brainstorms, and relationship-building conversations.",
    ),
    (
        "discovery-call.md",
        "Client/prospect conversations focused on needs, fit, evidence, and next steps.",
    ),
    (
        "group-conversation.md",
        "Three or more participants where who thought or committed what matters.",
    ),
    (
        "design-scoping-session.md",
        "Architecture, design, or technical scoping sessions where the shape of the problem and the concrete building blocks matter more than who-thought-what.",
    ),
    (
        "talk-reflection.md",
        "Talks, sermons, lectures, panels, and listening/reflection sessions where the memo captures the user's response.",
    ),
];

/// Resolve where templates are read from. The bundled template directory shipped
/// alongside the skill is the source of truth, so updates to bundled templates always
/// take effect. A per-session `<margins-dir>/templates/` directory is honored only as an
/// explicit user override and is never auto-seeded — auto-seeding turned that directory
/// into a stale cache that shadowed bundle updates forever.
fn resolve_template_dirs(
    margins_dir: &Path,
    skill_path: &Path,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let skill_dir = skill_path.parent().ok_or_else(|| {
        "could not resolve the bundled Margins template directory from the skill path".to_string()
    })?;
    let bundle_dir = if skill_dir.join("templates").exists() {
        skill_dir.join("templates")
    } else {
        skill_dir
            .parent()
            .map(|root| root.join("templates"))
            .ok_or_else(|| {
                "could not resolve the bundled Margins template directory from the skill path"
                    .to_string()
            })?
    };
    if !bundle_dir.exists() {
        return Err(format!(
            "bundled Margins template directory not found at {}. Reinstall Margins.",
            bundle_dir.display()
        ));
    }

    let override_dir = margins_dir.join("templates");
    let override_dir = if override_dir.exists() {
        Some(override_dir)
    } else {
        None
    };

    Ok((bundle_dir, override_dir))
}

fn resolve_distillation_core_path(skill_path: &Path) -> Result<PathBuf, String> {
    let skill_dir = skill_path.parent().ok_or_else(|| {
        "could not resolve the shared Margins distillation core from the skill path".to_string()
    })?;
    let core = if skill_dir.file_name().and_then(|name| name.to_str()) == Some("hosts") {
        skill_dir
            .parent()
            .map(|root| root.join("distillation-core.md"))
            .ok_or_else(|| {
                "could not resolve the shared Margins distillation core from the host preamble path"
                    .to_string()
            })?
    } else {
        skill_dir.join("distillation-core.md")
    };
    if core.exists() {
        Ok(core)
    } else {
        Err(format!(
            "shared Margins distillation core not found at {}. Reinstall Margins.",
            core.display()
        ))
    }
}

/// Load each template, preferring a user override file in `override_dir` when present and
/// otherwise reading from the bundle. Nothing is copied, so the bundle stays authoritative.
fn load_margins_template_bundle(bundle_dir: &Path, override_dir: Option<&Path>) -> String {
    let mut out = String::new();
    for (filename, purpose) in MARGINS_TEMPLATE_SPECS {
        out.push_str(&format!("## `{filename}`\n\nPurpose: {purpose}\n\n"));
        let override_path = override_dir.map(|d| d.join(filename));
        let path = match override_path {
            Some(p) if p.exists() => p,
            _ => bundle_dir.join(filename),
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                out.push_str(content.trim());
                out.push_str("\n\n");
            }
            Err(e) => {
                out.push_str(&format!(
                    "Could not load this template from {}: {e}\n\n",
                    path.display()
                ));
            }
        }
    }
    out.trim_end().to_string()
}

fn common_ancestor(a: &Path, b: &Path) -> Option<PathBuf> {
    let a = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let b = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    let mut out = PathBuf::new();
    for (left, right) in a.components().zip(b.components()) {
        if left != right {
            break;
        }
        out.push(left.as_os_str());
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

struct MarginsPiToolFactory {
    vault_path: Option<PathBuf>,
}

impl ToolFactory for MarginsPiToolFactory {
    fn create_tool_registry(&self, enabled: &[&str], cwd: &Path, config: &Config) -> ToolRegistry {
        let mut registry = default_tool_registry(enabled, cwd, config);
        if enabled.contains(&"enzyme_petri") {
            registry.push(Box::new(EnzymePetriTool {
                vault_path: self.vault_path.clone(),
            }));
        }
        if enabled.contains(&"enzyme_catalyze") {
            registry.push(Box::new(EnzymeSearchTool {
                vault_path: self.vault_path.clone(),
            }));
        }
        registry
    }
}

struct EnzymePetriTool {
    vault_path: Option<PathBuf>,
}

#[derive(Deserialize)]
struct EnzymePetriInput {
    query: Option<String>,
    top: Option<u32>,
    catalyst_budget: Option<u32>,
}

#[async_trait]
impl Tool for EnzymePetriTool {
    fn name(&self) -> &str {
        "enzyme_petri"
    }
    fn label(&self) -> &str {
        "Explore Notes"
    }
    fn description(&self) -> &str {
        "Rank themes from the configured notes folder against a capture-specific query."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Memo/transcript-derived topic query used to rank note themes for this capture.",
                    "minLength": 8
                },
                "top": {
                    "type": "integer",
                    "description": "Maximum ranked entities to return. Defaults to 8.",
                    "default": 8,
                    "minimum": 1,
                    "maximum": 12
                },
                "catalyst_budget": {
                    "type": "integer",
                    "description": "Maximum catalysts per entity in query mode. Defaults to 2.",
                    "default": 2,
                    "minimum": 1,
                    "maximum": 3
                }
            },
            "required": ["query"]
        })
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        _input: Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> pi::sdk::Result<ToolOutput> {
        let parsed: EnzymePetriInput = serde_json::from_value(_input)
            .map_err(|e| pi::sdk::Error::validation(e.to_string()))?;
        let query = parsed.query.unwrap_or_default().trim().to_string();
        if query.is_empty() {
            return Err(pi::sdk::Error::validation(
                "Vault exploration requires a non-empty memo/transcript-derived query; unqueried whole-vault output is intentionally disabled for Margins distillation.".to_string(),
            ));
        }
        let top = parsed.top.unwrap_or(8).clamp(1, 12).to_string();
        let catalyst_budget = parsed.catalyst_budget.unwrap_or(2).clamp(1, 3).to_string();
        let output = match &self.vault_path {
            // Enzyme is operated at the PROJECT ROOT (`self.vault_path`), with that
            // root as the working directory so the root-level `.enzyme` context is
            // used rather than the notes/inbox subfolder.
            Some(vault) => {
                let mut args = vec![
                    "petri".to_string(),
                    "--vault".to_string(),
                    vault.to_string_lossy().to_string(),
                    "--top".to_string(),
                    top,
                    "--catalyst-budget".to_string(),
                    catalyst_budget,
                ];
                args.push("--query".to_string());
                args.push(query);
                run_enzyme(args, Some(vault.as_path()))
            }
            None => Err(enzyme_error_output(
                "No notes folder configured in Margins settings",
            )),
        };
        Ok(enzyme_tool_output(output))
    }
}

#[derive(Deserialize)]
struct EnzymeSearchInput {
    query: String,
    limit: Option<u32>,
}

struct EnzymeSearchTool {
    vault_path: Option<PathBuf>,
}

#[async_trait]
impl Tool for EnzymeSearchTool {
    fn name(&self) -> &str {
        "enzyme_catalyze"
    }
    fn label(&self) -> &str {
        "Search Related Notes"
    }
    fn description(&self) -> &str {
        "Search the configured notes folder for semantically related passages."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Substantive search query using language from the conversation and notes folder." },
                "limit": { "type": "integer", "description": "Maximum number of results", "default": 8 }
            },
            "required": ["query"]
        })
    }
    async fn execute(
        &self,
        _tool_call_id: &str,
        input: Value,
        _on_update: Option<Box<dyn Fn(ToolUpdate) + Send + Sync>>,
    ) -> pi::sdk::Result<ToolOutput> {
        let parsed: EnzymeSearchInput =
            serde_json::from_value(input).map_err(|e| pi::sdk::Error::validation(e.to_string()))?;
        let limit = parsed.limit.unwrap_or(8).clamp(1, 20).to_string();
        let output = match &self.vault_path {
            // Operate enzyme at the PROJECT ROOT, same as the explore tool.
            Some(vault) => run_enzyme(
                vec![
                    "catalyze".to_string(),
                    "--vault".to_string(),
                    vault.to_string_lossy().to_string(),
                    "--limit".to_string(),
                    limit,
                    parsed.query,
                ],
                Some(vault.as_path()),
            ),
            None => Err(enzyme_error_output(
                "No notes folder configured in Margins settings",
            )),
        };
        Ok(enzyme_tool_output(output))
    }
}

fn strip_margins_markers(markdown: &str) -> (String, Option<Vec<SessionGrounding>>) {
    let mut clean = Vec::new();
    let mut uses = Vec::new();
    let mut suppress_visible_grounding = false;
    for line in markdown.lines() {
        let trimmed = line.trim();
        if is_visible_grounding_heading(trimmed) {
            suppress_visible_grounding = true;
            continue;
        }
        if suppress_visible_grounding {
            if is_markdown_heading(trimmed) {
                suppress_visible_grounding = false;
            } else {
                continue;
            }
        }
        if trimmed.starts_with("<!--MARGINS:") || trimmed.starts_with("<!-- MARGINS:") {
            if let Some(rest) = trimmed
                .strip_prefix("<!--MARGINS:USE")
                .or_else(|| trimmed.strip_prefix("<!-- MARGINS:USE"))
            {
                if let Some(json_text) = rest.strip_suffix("-->") {
                    if let Ok(value) = serde_json::from_str::<Value>(json_text.trim()) {
                        if let Some(grounding) = session_grounding_from_marker(&value) {
                            uses.push(grounding);
                        }
                    }
                }
            }
            continue;
        }
        clean.push(line);
    }
    let grounding = if uses.is_empty() { None } else { Some(uses) };
    (clean.join("\n"), grounding)
}

fn session_grounding_from_marker(value: &Value) -> Option<SessionGrounding> {
    let memo_ids = value
        .get("memo_ids")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|id| id.starts_with('m') && id.len() >= 4)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if memo_ids.is_empty() {
        return None;
    }
    let note_quote = value
        .get("note_quote")
        .and_then(Value::as_str)
        .or_else(|| value.get("quote").and_then(Value::as_str))
        .map(str::trim)
        .filter(|quote| !quote.is_empty())?
        .to_string();
    Some(SessionGrounding {
        memo_ids,
        note_quote,
        section_id: value
            .get("section_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        disposition: value
            .get("disposition")
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

fn is_visible_grounding_heading(trimmed: &str) -> bool {
    let lower = trimmed.trim_end_matches(':').to_ascii_lowercase();
    if lower == "grounding" || lower == "sources" {
        return true;
    }
    let Some(rest) = lower.strip_prefix('#') else {
        return false;
    };
    rest.trim_start_matches('#')
        .trim_start()
        .starts_with("grounding")
}

fn is_markdown_heading(trimmed: &str) -> bool {
    trimmed.starts_with('#')
        && trimmed.chars().take_while(|c| *c == '#').count() < trimmed.len()
        && trimmed.contains(' ')
}

struct EnzymeRunOutput {
    stdout: String,
    stderr: String,
    duration_ms: u128,
    exit_code: Option<i32>,
    command: Vec<String>,
}

// In-process petri/catalyze via the vendored recall engine (no subprocess). The
// arg vector and returned JSON envelope match the former external `enzyme`
// invocation, so callers and their parsers are unchanged.
fn run_enzyme(
    args: Vec<String>,
    _work_dir: Option<&Path>,
) -> Result<EnzymeRunOutput, EnzymeRunOutput> {
    let command = args.clone();
    let started = Instant::now();
    match crate::recall_search_compat(&args) {
        Ok(stdout) => Ok(EnzymeRunOutput {
            stdout,
            stderr: String::new(),
            duration_ms: started.elapsed().as_millis(),
            exit_code: Some(0),
            command,
        }),
        Err(stderr) => Err(EnzymeRunOutput {
            stdout: String::new(),
            stderr,
            duration_ms: started.elapsed().as_millis(),
            exit_code: Some(1),
            command,
        }),
    }
}

fn enzyme_tool_output(output: Result<EnzymeRunOutput, EnzymeRunOutput>) -> ToolOutput {
    match output {
        Ok(run) => ToolOutput {
            content: vec![ContentBlock::Text(TextContent::new(
                if run.stdout.trim().is_empty() {
                    "(no results)".to_string()
                } else {
                    run.stdout.clone()
                },
            ))],
            details: Some(enzyme_run_details(&run)),
            is_error: false,
        },
        Err(run) => ToolOutput {
            content: vec![ContentBlock::Text(TextContent::new(format!(
                "recall lookup failed: {}{}",
                run.stderr, run.stdout
            )))],
            details: Some(enzyme_run_details(&run)),
            is_error: true,
        },
    }
}

fn enzyme_run_details(run: &EnzymeRunOutput) -> Value {
    json!({
        "command": run.command,
        "duration_ms": run.duration_ms,
        "exit_code": run.exit_code,
        "stdout_bytes": run.stdout.len(),
        "stderr_bytes": run.stderr.len(),
        "stderr_tail": truncate_for_details(&run.stderr, 500),
    })
}

fn enzyme_error_output(message: &str) -> EnzymeRunOutput {
    EnzymeRunOutput {
        stdout: String::new(),
        stderr: message.to_string(),
        duration_ms: 0,
        exit_code: None,
        command: Vec::new(),
    }
}

fn truncate_for_details(input: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (i, ch) in input.chars().enumerate() {
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn note_filename_from_config(
    config: &NoteConfig,
    fallback: &str,
    descriptive_title: Option<&str>,
) -> String {
    let title = descriptive_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .or(config.event_title.as_deref())
        .unwrap_or(fallback);
    let dt = note_datetime(config).unwrap_or_else(Local::now);
    let mut out = config.note_filename_template.clone();
    while let Some(start) = out.find("{{date:") {
        if let Some(rel_end) = out[start..].find("}}") {
            let end = start + rel_end + 2;
            let fmt = &out[start + 7..end - 2];
            out.replace_range(start..end, &dt.format(fmt).to_string());
        } else {
            break;
        }
    }
    out = out
        .replace("{{event_title}}", &title.to_lowercase())
        .replace("{{title}}", &title.to_lowercase());
    let safe = safe_markdown_filename(&out);
    if safe.is_empty() {
        format!("{fallback}.md")
    } else {
        safe
    }
}

fn note_title_for_filename(markdown: &str) -> Option<String> {
    crate::note_artifacts::parse_note_frontmatter(markdown)
        .title
        .filter(|title| !title.trim().is_empty())
        .or_else(|| {
            markdown.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("# ")
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .map(ToString::to_string)
            })
        })
}

fn people_from_frontmatter_map(map: &Map<String, Value>) -> Vec<String> {
    let mut people = Vec::new();
    for key in ["people", "person", "attendees"] {
        let Some(value) = map.get(key) else {
            continue;
        };
        match value {
            Value::Array(items) => {
                for item in items {
                    match item {
                        Value::String(s) => people.push(s.clone()),
                        other => people.push(other.to_string()),
                    }
                }
            }
            Value::String(s) => {
                if s.trim_start().starts_with('[') && s.trim_end().ends_with(']') {
                    people.extend(
                        s.trim()
                            .trim_start_matches('[')
                            .trim_end_matches(']')
                            .split(',')
                            .map(str::to_string),
                    );
                } else {
                    people.push(s.clone());
                }
            }
            other => people.push(other.to_string()),
        }
    }
    people
}

fn merge_people(primary: &[String], additional: &[String]) -> Vec<String> {
    let mut merged = Vec::new();
    for raw in primary.iter().chain(additional.iter()) {
        let person = clean_person_name(raw);
        if person.is_empty() {
            continue;
        }
        let key = person.to_ascii_lowercase();
        if !merged
            .iter()
            .any(|existing: &String| existing.to_ascii_lowercase() == key)
        {
            merged.push(person);
        }
    }
    merged
}

fn clean_person_name(raw: &str) -> String {
    let mut value = raw.trim().trim_matches(',').trim().to_string();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value = value[1..value.len() - 1].trim().to_string();
    }
    if let Some(inner) = value.strip_prefix("[[").and_then(|v| v.strip_suffix("]]")) {
        value = inner.split('|').next().unwrap_or(inner).trim().to_string();
    }
    value
}

fn ensure_note_frontmatter(markdown: &str, config: &NoteConfig, session_name: &str) -> String {
    let created_value = note_datetime(config)
        .unwrap_or_else(Local::now)
        .format(&config.created_date_format)
        .to_string();
    let normalized = markdown.replace("\r\n", "\n");
    if let Some(rest) = normalized.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let fm_text = &rest[..end];
            let body = &rest[end + 4..];
            let mut map = crate::note_artifacts::serde_yaml_like_frontmatter(fm_text);
            map.entry("created".to_string())
                .or_insert(Value::String(created_value));
            stamp_margins_session(&mut map, session_name);
            let existing_people = people_from_frontmatter_map(&map);
            let merged_people = merge_people(&config.people, &existing_people);
            if !merged_people.is_empty() {
                map.insert(
                    "people".to_string(),
                    Value::Array(
                        merged_people
                            .into_iter()
                            .map(|p| Value::String(format!("[[{p}]]")))
                            .collect(),
                    ),
                );
            }
            return format!(
                "---\n{}---{}",
                crate::note_artifacts::render_frontmatter_map(&map),
                body
            );
        }
    }
    let people_values: Vec<String> = merge_people(&config.people, &[])
        .into_iter()
        .map(|p| format!("[[{}]]", p))
        .collect();
    let mut fm = Map::new();
    fm.insert("tags".to_string(), Value::Array(Vec::new()));
    if !people_values.is_empty() {
        fm.insert(
            "people".to_string(),
            Value::Array(people_values.into_iter().map(Value::String).collect()),
        );
    } else {
        fm.insert("people".to_string(), Value::Array(Vec::new()));
    }
    fm.insert("created".to_string(), Value::String(created_value));
    stamp_margins_session(&mut fm, session_name);
    format!(
        "---\n{}---\n{}",
        crate::note_artifacts::render_frontmatter_map(&fm),
        normalized.trim_start()
    )
}

/// Stamp a durable `margins_session: <session name>` backlink into the note's
/// frontmatter so the note↔session link survives renames and DB resets. Only
/// fills the key when absent and the name is non-empty — never clobbers a value
/// an existing note (e.g. one being refined in place) already carries.
fn stamp_margins_session(map: &mut Map<String, Value>, session_name: &str) {
    let name = session_name.trim();
    if name.is_empty() {
        return;
    }
    map.entry("margins_session".to_string())
        .or_insert_with(|| Value::String(name.to_string()));
}

fn note_datetime(config: &NoteConfig) -> Option<DateTime<Local>> {
    config
        .event_start
        .as_deref()
        .and_then(parse_note_datetime_local)
}

fn parse_note_datetime_local(value: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Local))
        .ok()
        .or_else(|| parse_note_naive_datetime_local(value))
        .or_else(|| parse_note_date_local(value))
}

fn parse_note_naive_datetime_local(value: &str) -> Option<DateTime<Local>> {
    let naive = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S"))
        .ok()?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt),
        LocalResult::Ambiguous(earliest, _) => Some(earliest),
        LocalResult::None => None,
    }
}

fn parse_note_date_local(value: &str) -> Option<DateTime<Local>> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    match Local.from_local_datetime(&date.and_hms_opt(0, 0, 0)?) {
        LocalResult::Single(dt) => Some(dt),
        LocalResult::Ambiguous(earliest, _) => Some(earliest),
        LocalResult::None => None,
    }
}

fn unique_available_note_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("note");
    let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("md");
    for ordinal in 2..1000 {
        let candidate = parent.join(format!("{stem}-{ordinal}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}-{}.{extension}", Local::now().timestamp()))
}

fn ensure_people_files(vault: &Path, config: &NoteConfig) -> std::io::Result<()> {
    crate::note_artifacts::ensure_people_files(
        vault,
        &config.people_folder,
        &config.person_note_template,
        &config.people,
    )
    .map_err(std::io::Error::other)
}

fn safe_markdown_filename(input: &str) -> String {
    let stem = input.trim().trim_end_matches(".md");
    let mut out = String::new();
    let mut last_was_space = false;
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
            last_was_space = false;
        } else if ch.is_whitespace() {
            if !last_was_space && !out.is_empty() {
                out.push(' ');
                last_was_space = true;
            }
        } else if ch == '/' || ch == '\\' {
            out.push('-');
            last_was_space = false;
        }
    }
    let out = out.trim();
    if out.is_empty() {
        String::new()
    } else {
        format!("{out}.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_draft_keeps_exact_delta_order_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "margins-stream-draft-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path;
        {
            let mut draft = StreamingNoteDraft::new(&dir, "stream-test");
            path = draft.path.clone();
            draft.push("first ");
            draft.push("second\n");
            assert_eq!(draft.buffer(), "first second\n");
        }
        assert_eq!(std::fs::read_to_string(path).unwrap(), "first second\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "margins-pi-distill-test-{name}-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn find_file_with_extension(root: &Path, extension: &str) -> Option<PathBuf> {
        for entry in std::fs::read_dir(root).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = find_file_with_extension(&path, extension) {
                    return Some(found);
                }
            } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                return Some(path);
            }
        }
        None
    }

    #[test]
    fn margins_pi_call_path_forces_jsonl_and_resumes_despite_global_sqlite_config() {
        let dir = temp_dir("explicit-jsonl-resume");
        let pi_home = dir.join("pi-home");
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&pi_home).unwrap();
        std::fs::write(
            pi_home.join("settings.json"),
            r#"{"sessionStore":"sqlite"}"#,
        )
        .unwrap();
        let previous_pi_home = std::env::var_os("PI_CODING_AGENT_DIR");
        std::env::set_var("PI_CODING_AGENT_DIR", &pi_home);

        let reactor = asupersync::runtime::reactor::create_reactor().unwrap();
        let runtime = asupersync::runtime::RuntimeBuilder::current_thread()
            .with_reactor(reactor)
            .build()
            .unwrap();
        let result = runtime.block_on(async {
            let base_options = SessionOptions {
                provider: Some("openai".to_string()),
                model: Some("gpt-4o".to_string()),
                api_key: Some("loopback-only-test-key".to_string()),
                working_directory: Some(dir.clone()),
                no_session: false,
                session_dir: Some(sessions.clone()),
                enabled_tools: Some(Vec::new()),
                ..SessionOptions::default()
            };
            let mut first =
                create_agent_session_with_store(base_options.clone(), SessionStoreKind::Jsonl)
                    .await?;
            let first_id = first.state().await?.session_id.expect("new session id");
            first.set_session_name("Margins JSONL fixture").await?;
            drop(first);

            let path = find_file_with_extension(&sessions, "jsonl")
                .expect("explicit Margins call should persist JSONL");
            let resumed = create_agent_session_with_store(
                SessionOptions {
                    session_path: Some(path.clone()),
                    ..base_options
                },
                SessionStoreKind::Jsonl,
            )
            .await?;
            let resumed_state = resumed.state().await?;
            anyhow::ensure!(resumed_state.session_id.as_deref() == Some(first_id.as_str()));
            anyhow::ensure!(resumed_state.message_count == 0);
            Ok::<PathBuf, anyhow::Error>(path)
        });

        match previous_pi_home {
            Some(value) => std::env::set_var("PI_CODING_AGENT_DIR", value),
            None => std::env::remove_var("PI_CODING_AGENT_DIR"),
        }
        let path = result.unwrap();
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("jsonl")
        );
        assert!(
            find_file_with_extension(&sessions, "sqlite").is_none(),
            "embedded Pi must not create a SQLite conversation"
        );
        assert!(
            !sessions.join("session-index.sqlite").exists(),
            "default-features=false must omit Pi's session index"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn chooses_ordinal_note_path_when_file_exists() {
        let dir = temp_dir("ordinal-note-path");
        let requested = dir.join("customer-call.md");
        std::fs::write(&requested, "existing").unwrap();
        assert_eq!(
            unique_available_note_path(&requested),
            dir.join("customer-call-2.md")
        );
        std::fs::write(dir.join("customer-call-2.md"), "existing").unwrap();
        assert_eq!(
            unique_available_note_path(&requested),
            dir.join("customer-call-3.md")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn note_frontmatter_created_uses_event_start_when_available() {
        let config = NoteConfig {
            created_date_format: "%Y-%m-%d".to_string(),
            event_start: Some("2026-06-01".to_string()),
            ..NoteConfig::default()
        };
        let rendered = ensure_note_frontmatter("# Meeting", &config, "morning-sync");
        assert!(rendered.contains("created: '2026-06-01'"));
        assert!(rendered.contains("margins_session: 'morning-sync'"));
    }

    #[test]
    fn note_filename_uses_event_start_and_title() {
        let config = NoteConfig {
            note_filename_template: "{{date:%Y-%m-%d}} {{event_title}}".to_string(),
            event_title: Some("Customer Call".to_string()),
            event_start: Some("2026-06-01".to_string()),
            ..NoteConfig::default()
        };
        assert_eq!(
            note_filename_from_config(&config, "fallback", None),
            "2026-06-01 customer call.md"
        );
    }

    #[test]
    fn participant_guidance_names_single_configured_person() {
        let config = NoteConfig {
            people: vec!["Timour".to_string()],
            ..NoteConfig::default()
        };

        let guidance = participant_guidance_text(&config);

        assert!(guidance.contains("Timour"));
        assert!(guidance.contains("What Timour thinks"));
    }

    #[test]
    fn catalyze_queries_include_people_and_event_context() {
        let dir = temp_dir("people-query-plan");
        let request = PiDistillRequest {
            work_dir: dir.clone(),
            margins_dir: dir.join(".margins"),
            trace_dir: dir.join(".margins/traces"),
            session_name: "agent-commons".to_string(),
            memo_path: dir.join("memo.md"),
            capture_context: String::new(),
            aligned_context: "Agent Commons research and Agent Village experiment".to_string(),
            vault_path: Some(dir.clone()),
            note_config: NoteConfig {
                people: vec!["Timour".to_string()],
                event_title: Some("Agent Commons research".to_string()),
                ..NoteConfig::default()
            },
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: None,
            prep_ai_provider: None,
            prep_ai_model: None,
            prep_ai_api_key: None,
            prep_ai_credential_generation: None,
            skill_path: dir.join("skill.md"),
            cancel: Arc::new(AtomicBool::new(false)),
            resume_session_path: None,
            refine_message: None,
            existing_note_path: None,
            save_generated_note: false,
        };

        let queries = normalize_catalyze_queries(
            vec!["Agent Commons community contribution".to_string()],
            "Agent Village experiment",
            &request,
        );

        assert_eq!(catalyze_query_limit(&request, queries.len()), 4);
        assert!(queries
            .iter()
            .any(|query| query.contains("Agent Commons research")));
        assert!(queries.iter().any(|query| query.contains("Timour")));
        assert_eq!(queries.len(), 4);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn loads_templates_from_bundle_and_honors_user_override() {
        let dir = temp_dir("templates");
        let margins_dir = dir.join(".margins");
        let skill_dir = dir.join("skill");
        let bundled_template_dir = skill_dir.join("templates");
        std::fs::create_dir_all(&bundled_template_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(&skill_path, "# skill").unwrap();
        for (filename, _) in MARGINS_TEMPLATE_SPECS {
            std::fs::write(
                bundled_template_dir.join(filename),
                format!("# bundled {filename}\n"),
            )
            .unwrap();
        }
        // A user override exists for only one template; the rest must come from the bundle.
        std::fs::create_dir_all(margins_dir.join("templates")).unwrap();
        std::fs::write(
            margins_dir.join("templates/discovery-call.md"),
            "# custom discovery template\n\n### Custom Needs",
        )
        .unwrap();

        let (bundle_dir, override_dir) = resolve_template_dirs(&margins_dir, &skill_path).unwrap();
        let bundle = load_margins_template_bundle(&bundle_dir, override_dir.as_deref());

        // Bundle is the source of truth; nothing is seeded/copied into the override dir.
        assert_eq!(bundle_dir, bundled_template_dir);
        assert_eq!(override_dir, Some(margins_dir.join("templates")));
        assert!(!margins_dir.join("templates/1on1-idea-exchange.md").exists());

        assert!(bundle.starts_with("## `1on1-idea-exchange.md`"));
        assert!(bundle.contains("Default for most 1:1 conversations"));
        // 1on1 loaded from the bundle, discovery-call from the user override.
        assert!(bundle.contains("# bundled 1on1-idea-exchange.md"));
        assert!(bundle.contains("# custom discovery template"));
        assert!(!bundle.contains("# bundled discovery-call.md"));
        assert!(!bundle.contains("Could not load this template"));

        // With no override dir at all, every template resolves from the bundle.
        let no_override_margins = dir.join("empty-margins");
        let (bundle_dir2, override_dir2) =
            resolve_template_dirs(&no_override_margins, &skill_path).unwrap();
        let bundle2 = load_margins_template_bundle(&bundle_dir2, override_dir2.as_deref());
        assert_eq!(override_dir2, None);
        assert!(bundle2.contains("# bundled discovery-call.md"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn host_preamble_resolves_shared_core_and_templates_from_skill_root() {
        let dir = temp_dir("host-skill");
        let margins_dir = dir.join(".margins");
        let skill_root = dir.join("margins");
        let host_dir = skill_root.join("hosts");
        let bundled_template_dir = skill_root.join("templates");
        std::fs::create_dir_all(&host_dir).unwrap();
        std::fs::create_dir_all(&bundled_template_dir).unwrap();
        let host_path = host_dir.join("desktop.md");
        let core_path = skill_root.join("distillation-core.md");
        std::fs::write(&host_path, "# desktop host").unwrap();
        std::fs::write(&core_path, "# shared core").unwrap();
        for (filename, _) in MARGINS_TEMPLATE_SPECS {
            std::fs::write(
                bundled_template_dir.join(filename),
                format!("# bundled {filename}\n"),
            )
            .unwrap();
        }

        let (bundle_dir, override_dir) = resolve_template_dirs(&margins_dir, &host_path).unwrap();

        assert_eq!(
            resolve_distillation_core_path(&host_path).unwrap(),
            core_path
        );
        assert_eq!(bundle_dir, bundled_template_dir);
        assert_eq!(override_dir, None);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn merges_people_from_source_note_frontmatter_into_note_config() {
        let config = NoteConfig {
            people: vec!["Ada Lovelace".to_string()],
            ..NoteConfig::default()
        };
        let memo = r#"---
people:
  - "[[Grace Hopper]]"
attendees: ["[[Linus Torvalds]]", "Ada Lovelace"]
---
[00:01] memo body
"#;

        let merged = note_config_with_source_frontmatter(config, memo);

        assert_eq!(
            merged.people,
            vec!["Ada Lovelace", "Grace Hopper", "Linus Torvalds"]
        );
    }

    #[test]
    fn ensure_note_frontmatter_merges_config_people_with_model_people() {
        let config = NoteConfig {
            people: vec!["Ada Lovelace".to_string()],
            ..NoteConfig::default()
        };
        let markdown = r#"---
tags:
  - "meeting"
people:
  - "[[Grace Hopper]]"
---
# Body
"#;

        let out = ensure_note_frontmatter(markdown, &config, "session-x");
        let parsed = crate::note_artifacts::parse_note_frontmatter(&out);

        assert_eq!(parsed.people, vec!["Ada Lovelace", "Grace Hopper"]);
        assert_eq!(parsed.tags, vec!["meeting"]);
        assert!(out.contains("\"[[Ada Lovelace]]\""));
        assert!(out.contains("\"[[Grace Hopper]]\""));
    }

    #[test]
    fn fallback_frontmatter_omits_reflection_type() {
        let config = NoteConfig {
            created_date_format: "%Y-%m-%d".to_string(),
            event_start: Some("2026-06-01".to_string()),
            people: vec!["Ada Lovelace".to_string()],
            ..NoteConfig::default()
        };
        // No frontmatter in the input — triggers the fallback branch
        let rendered = ensure_note_frontmatter("# Meeting notes", &config, "weekly-1on1");
        assert!(
            rendered.starts_with("---\n"),
            "should start with frontmatter"
        );
        assert!(rendered.contains("created:"), "should contain created key");
        assert!(rendered.contains("tags:"), "should contain tags key");
        assert!(rendered.contains("people:"), "should contain people key");
        assert!(
            !rendered.contains("reflectionType"),
            "fallback must not inject reflectionType"
        );
    }

    #[test]
    fn ensure_note_frontmatter_parses_inline_people_arrays_before_merging() {
        let config = NoteConfig {
            people: vec!["Ada Lovelace".to_string()],
            ..NoteConfig::default()
        };
        let markdown = r#"---
people: ["[[Grace Hopper]]", "[[Linus Torvalds]]"]
---
# Body
"#;

        let out = ensure_note_frontmatter(markdown, &config, "session-x");
        let parsed = crate::note_artifacts::parse_note_frontmatter(&out);

        assert_eq!(
            parsed.people,
            vec!["Ada Lovelace", "Grace Hopper", "Linus Torvalds"]
        );
    }

    #[test]
    fn ensure_note_frontmatter_preserves_existing_margins_session() {
        let config = NoteConfig::default();
        // A note being refined in place already carries its original backlink;
        // re-stamping must not overwrite it with the (possibly different) name.
        let markdown = r#"---
margins_session: original-session
---
# Body
"#;
        let out = ensure_note_frontmatter(markdown, &config, "refine-call");
        let parsed = crate::note_artifacts::parse_note_frontmatter(&out);
        assert_eq!(parsed.margins_session.as_deref(), Some("original-session"));
    }

    #[test]
    fn ensure_note_frontmatter_skips_margins_session_when_name_empty() {
        let config = NoteConfig::default();
        let out = ensure_note_frontmatter("# Body", &config, "  ");
        assert!(!out.contains("margins_session"));
    }

    #[test]
    fn summarize_catalyze_stdout_uses_typed_evidence_and_stale_signal() {
        let summary = summarize_catalyze_stdout(
            r#"{"schema_version":"margins.recall.v1","status":"ok","freshness":{"status":"stale","stale":true,"reason":"refresh failed","index":{"status":"stale","stale":true,"reason":"refresh failed"},"materialization":[]},"results":[{"document_ref":"meetings/pilot.md","source_kind":"notes","evidence":{"kind":"native_markdown","path":"/vault/meetings/pilot.md"},"content":"prior rollout context","similarity":0.82},{"document_ref":"mail:abc","source_kind":"google-mail","evidence":{"kind":"external_record","connector_id":"email","source_account":"owner@example.com","source_id":"abc"},"content":"email thread context","similarity":0.71}]}"#,
        );
        assert!(summary.contains("[stale evidence: refresh failed]"));
        assert!(summary.contains("### pilot"));
        assert!(summary.contains("### email thread abc"));
        assert!(!summary.contains("file_path"));
    }

    #[test]
    fn pi_provider_error_checks_cancellation_at_error_boundary() {
        let cancelled_generation = crate::ai_config::install_test_included_lease_generation();
        let cancelled_request = provider_error_test_request(Some(cancelled_generation), None, true);
        assert_eq!(
            handle_pi_provider_error(&cancelled_request, PiCredentialLane::Final, "distill_setup",),
            crate::ai_config::ProviderErrorDisposition::Cancelled
        );
        assert!(crate::ai_config::test_included_lease_generation_is_usable(
            cancelled_generation
        ));

        let failed_generation = crate::ai_config::install_test_included_lease_generation();
        let failed_request = provider_error_test_request(None, Some(failed_generation), false);
        assert_eq!(
            handle_pi_provider_error(&failed_request, PiCredentialLane::Prep, "prep_prompt",),
            crate::ai_config::ProviderErrorDisposition::Invalidated
        );
        assert!(!crate::ai_config::test_included_lease_generation_is_usable(
            failed_generation
        ));
    }

    fn provider_error_test_request(
        final_generation: Option<u64>,
        prep_generation: Option<u64>,
        cancelled: bool,
    ) -> PiDistillRequest {
        let dir = temp_dir("provider-error-generation");
        PiDistillRequest {
            work_dir: dir.clone(),
            margins_dir: dir.join(".margins"),
            trace_dir: dir.join(".margins/traces"),
            session_name: "generation-test".to_string(),
            memo_path: dir.join("memo.md"),
            capture_context: String::new(),
            aligned_context: String::new(),
            vault_path: None,
            note_config: NoteConfig::default(),
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: final_generation,
            prep_ai_provider: None,
            prep_ai_model: None,
            prep_ai_api_key: None,
            prep_ai_credential_generation: prep_generation,
            skill_path: dir.join("skill.md"),
            cancel: Arc::new(AtomicBool::new(cancelled)),
            resume_session_path: None,
            refine_message: None,
            existing_note_path: None,
            save_generated_note: false,
        }
    }
}
