use pi::sdk::{create_agent_session_with_store, SessionOptions, SessionStoreKind, ThinkingLevel};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::pi_events::{
    assistant_text, note_text_delta, recall_freshness_stale_prefix, recall_result_related_heading,
};

const BACKCHANNEL_MAX_TOKENS: u32 = 192;
const BACKCHANNEL_CATALYZE_RESULT_CHARS: usize = 600;
const BACKCHANNEL_CATALYZE_CONTEXT_CHARS: usize = 2_000;

const BACKCHANNEL_SYSTEM_PROMPT: &str = "You are the quiet backchannel inside Margins Desktop, watching a live conversation beside the user\u{2019}s margin notes. Your job is externalization: help the user say the thing they are already privately holding - the tension they marked, the question that is already alive in the room.

The shape of a cue (non-negotiable):
- ONE question or fragment, 12 words or fewer is ideal, NEVER more than 18.
- Half-built, not polished. The user finishes it in their own voice. A fragment beats a sentence. If it reads like a script or has two clauses, cut it down.
- Anchored to this moment: a phrase just used, a word doing extra work, the opening they almost took. Use what the vault context adds (a prior thread, the user\u{2019}s frameworks) over generic prompting.
- It draws the other person out. Never helps the user win, corner, or leverage private information.

Good cues look like:
- “Can I test something I’m noticing?”
- “when you said visibility - access, or permission to act?”
- “what’s actually slowing it down?”
- “are those two things separate for you?”

Bad cues look like (never produce these):
- “How do you decide which past ideas are worth surfacing, and does that change how you organize them?” (two clauses, consultant-polished, 18+ words)
- Any fully-quoted recitable sentence the user would read off a card.

Quiet gate: if the transcript is empty or nothing earns pulling the user’s eyes out of the room, stay quiet - unless the memo itself already holds a sayable question, in which case hand it back sharpened.

Do not write files. Return only strict JSON matching the requested schema.";

#[derive(Clone)]
pub(crate) struct BackchannelAiRequest {
    pub(crate) request_id: String,
    pub(crate) memo_index: usize,
    pub(crate) work_dir: PathBuf,
    pub(crate) margins_dir: PathBuf,
    pub(crate) session_name: String,
    pub(crate) memo_text: String,
    pub(crate) memo_time: String,
    pub(crate) transcript: String,
    pub(crate) catalyze_transcript: String,
    pub(crate) vault_path: Option<PathBuf>,
    pub(crate) ai_provider: Option<String>,
    pub(crate) ai_model: Option<String>,
    pub(crate) ai_api_key: Option<String>,
    pub(crate) ai_credential_generation: Option<u64>,
    pub(crate) instructions: String,
}

/// Frozen, reusable context for a single backchannel mark.
///
/// Computed once when the mark fires (catalyze search + notes folder), then
/// snapshotted so the user can steer the cue without re-running ASR/enzyme.
/// Reusing the identical prefix also keeps provider prompt-caching warm.
#[derive(Clone)]
pub(crate) struct BackchannelContext {
    pub(crate) queries: Vec<String>,
    pub(crate) catalyze_context: String,
    pub(crate) catalyze_error: Option<String>,
    pub(crate) notes_folder: String,
}

/// One round of steering: the user's instruction and the cue it produced.
/// Accumulated across a mark's lifetime so each new steer sees the full lineage.
#[derive(Clone)]
pub(crate) struct SteerExchange {
    pub(crate) steering: String,
    pub(crate) cue: String,
}

/// Build the cache-stable context (catalyze search + notes folder) for a mark.
/// Run once per mark; the result is snapshotted and reused for steering.
pub(crate) fn prepare_backchannel_context(request: &BackchannelAiRequest) -> BackchannelContext {
    let notes_folder = request
        .vault_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no notes folder configured)".to_string());

    let catalyze_input = build_catalyze_input(&request.memo_text, &request.catalyze_transcript);
    let queries = if catalyze_input.trim().is_empty() {
        Vec::new()
    } else {
        vec![catalyze_input.clone()]
    };
    let _ = append_backchannel_trace(
        request,
        json!({
            "kind":"related_note_catalyst_input",
            "input":catalyze_input.clone(),
            "memo":request.memo_text.clone(),
            "recent_transcript_words":recent_words(&request.catalyze_transcript, 50),
        }),
    );

    let catalyze = collect_catalyze_results(request.vault_path.as_ref(), &queries);
    let catalyze_context = catalyze.context;
    // A catalyze error can arrive two ways: as a parseable `{"error":...}`
    // payload, or as a hard process failure (spawn error / non-zero exit) that
    // never produced results JSON. Both must gate the model call. Routing the
    // hard-failure string through the same classifier means a non-zero exit
    // whose message says "not initialized" gets the honest unavailable state.
    let catalyze_error =
        catalyze_unavailable_reason(&catalyze_context).or_else(|| catalyze.hard_failure.clone());
    // Degraded index: catalyze ran without error yet every query was empty. Only
    // flag when there's no explicit error to attribute it to.
    let catalyze_degraded = catalyze_error.is_none() && catalyze.all_results_empty;
    let catalyze_questions = catalyze_question_context(&catalyze_context, &catalyze_input);
    let _ = append_backchannel_trace(
        request,
        json!({
            "kind":"related_note_context",
            "queries":queries.clone(),
            "context":catalyze_context.clone(),
            "error":catalyze_error.clone(),
            "degraded":catalyze_degraded,
            "question_context":catalyze_questions.clone(),
        }),
    );

    BackchannelContext {
        queries,
        catalyze_context,
        catalyze_error,
        notes_folder,
    }
}

pub(crate) fn backchannel_context_cache_key(request: &BackchannelAiRequest) -> Option<String> {
    let catalyze_input = build_catalyze_input(&request.memo_text, &request.catalyze_transcript);
    if catalyze_input.trim().is_empty() {
        return None;
    }
    let vault = request
        .vault_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no-notes-folder)".to_string());
    Some(format!("{vault}\n{catalyze_input}"))
}

pub(crate) fn run_backchannel_ai(
    request: BackchannelAiRequest,
    context: &BackchannelContext,
) -> Result<String, String> {
    run_backchannel_ai_blocking(request, context)
}

fn run_backchannel_ai_blocking(
    request: BackchannelAiRequest,
    context: &BackchannelContext,
) -> Result<String, String> {
    let queries = &context.queries;
    let catalyze_context = &context.catalyze_context;
    let notes_folder = &context.notes_folder;

    if let Some(reason) = context.catalyze_error.as_deref() {
        // Every recall failure is explicit. Quiet is reserved for a successful
        // catalyst search whose grounded model response has nothing to add.
        let raw = local_unavailable_setup(&request, queries, reason);
        let kind = "recall_unavailable";
        let _ = append_backchannel_trace(
            &request,
            json!({
                "kind":"suggestion_skipped",
                "reason":kind,
                "message":reason,
                "raw":raw.clone(),
            }),
        );
        return Ok(raw);
    }

    let final_system_prompt = BACKCHANNEL_SYSTEM_PROMPT.to_string();
    let final_prompt =
        build_suggestion_prompt(&request, notes_folder, queries, catalyze_context, None);
    let _ = append_backchannel_trace(
        &request,
        json!({
            "kind":"suggestion_prompt",
            "system_prompt":final_system_prompt.clone(),
            "prompt":final_prompt.clone(),
        }),
    );
    let suggestion_raw = prompt_text_model_blocking(
        &request,
        final_system_prompt,
        final_prompt,
        ThinkingLevel::Off,
    )?;
    let _ = append_backchannel_trace(
        &request,
        json!({
            "kind":"suggestion_response",
            "raw":suggestion_raw.clone(),
        }),
    );

    let mut parsed: Value = extract_json_object(&suggestion_raw)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_else(
            || json!({ "state": "suggestion_available", "suggestion": suggestion_raw }),
        );
    if let Some(obj) = parsed.as_object_mut() {
        obj.entry("search_queries".to_string())
            .or_insert_with(|| json!(queries));
    }
    serde_json::to_string(&parsed)
        .map_err(|e| format!("failed to encode backchannel response: {e}"))
}

/// Regenerate a single cue, steered by user feedback, reusing the frozen
/// context. No placeholder pass — the UI keeps showing the prior cue until this
/// resolves. The steer text is appended after the trigger details so the large
/// cache-stable prefix is byte-identical to the original request.
pub(crate) fn run_backchannel_steer(
    request: BackchannelAiRequest,
    context: &BackchannelContext,
    original_cue: Option<String>,
    history: &[SteerExchange],
    steering: String,
) -> Result<String, String> {
    let system_prompt = BACKCHANNEL_SYSTEM_PROMPT.to_string();
    let trailer = build_steer_trailer(original_cue.as_deref(), history, &steering);
    let prompt = build_suggestion_prompt(
        &request,
        &context.notes_folder,
        &context.queries,
        &context.catalyze_context,
        Some(trailer.as_str()),
    );
    let _ = append_backchannel_trace(
        &request,
        json!({
            "kind":"steer_prompt",
            "steering":steering,
            "original_cue":original_cue,
            "history_len":history.len(),
            "prompt":prompt.clone(),
        }),
    );
    let raw = prompt_text_model_blocking(&request, system_prompt, prompt, ThinkingLevel::Off)?;
    let _ = append_backchannel_trace(&request, json!({"kind":"steer_response","raw":raw.clone()}));

    let mut parsed: Value = extract_json_object(&raw)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_else(|| json!({ "state": "suggestion_available", "suggestion": raw }));
    if let Some(obj) = parsed.as_object_mut() {
        obj.entry("search_queries".to_string())
            .or_insert_with(|| json!(context.queries));
    }
    serde_json::to_string(&parsed)
        .map_err(|e| format!("failed to encode backchannel response: {e}"))
}

fn prompt_text_model_blocking(
    request: &BackchannelAiRequest,
    system_prompt: String,
    prompt: String,
    thinking: ThinkingLevel,
) -> Result<String, String> {
    let reactor = asupersync::runtime::reactor::create_reactor()
        .map_err(|e| format!("failed to create backchannel AI runtime: {e}"))?;
    let runtime = asupersync::runtime::RuntimeBuilder::current_thread()
        .with_reactor(reactor)
        .build()
        .map_err(|e| format!("failed to build backchannel AI runtime: {e}"))?;
    runtime
        .block_on(async move { prompt_text_model(request, system_prompt, prompt, thinking).await })
}

fn append_backchannel_trace(request: &BackchannelAiRequest, event: Value) -> Result<(), String> {
    std::fs::create_dir_all(&request.margins_dir).map_err(|e| e.to_string())?;
    let path = request
        .margins_dir
        .join(format!("{}_backchannel_trace.jsonl", request.session_name));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string())
    )
    .map_err(|e| e.to_string())
}

async fn prompt_text_model(
    request: &BackchannelAiRequest,
    system_prompt: String,
    prompt: String,
    thinking: ThinkingLevel,
) -> Result<String, String> {
    let session_create_started = Instant::now();
    let system_prompt_chars = system_prompt.chars().count();
    let prompt_chars = prompt.chars().count();
    let options = SessionOptions {
        provider: request.ai_provider.clone(),
        model: request.ai_model.clone(),
        api_key: request.ai_api_key.clone(),
        working_directory: Some(request.work_dir.clone()),
        no_session: true,
        session_dir: Some(request.margins_dir.join("ai-sessions")),
        enabled_tools: Some(Vec::new()),
        thinking: Some(thinking),
        max_tool_iterations: 0,
        append_system_prompt: Some(system_prompt),
        ..SessionOptions::default()
    };

    let mut session = create_agent_session_with_store(options, SessionStoreKind::Jsonl)
        .await
        .map_err(|e| {
            handle_backchannel_provider_error(request, "backchannel_setup");
            format!("backchannel AI setup failed: {e}")
        })?;
    let session_create_ms = session_create_started.elapsed().as_millis();
    session.set_max_tokens(Some(BACKCHANNEL_MAX_TOKENS));

    let prompt_started = Instant::now();
    let first_text_delta_ms = Arc::new(Mutex::new(None::<u128>));
    let first_text_delta_for_events = Arc::clone(&first_text_delta_ms);
    let assistant = session
        .prompt(prompt, move |event| {
            if note_text_delta(&event)
                .as_deref()
                .is_some_and(|delta| !delta.trim().is_empty())
            {
                if let Ok(mut first) = first_text_delta_for_events.lock() {
                    first.get_or_insert_with(|| prompt_started.elapsed().as_millis());
                }
            }
        })
        .await
        .map_err(|e| {
            handle_backchannel_provider_error(request, "backchannel_prompt");
            format!("backchannel AI failed: {e}")
        })?;
    let completion_ms = prompt_started.elapsed().as_millis();

    let text = assistant_text(&assistant.content).trim().to_string();
    if text.is_empty() {
        return Err("backchannel AI returned no content".to_string());
    }
    let first_text_delta_ms = first_text_delta_ms.lock().ok().and_then(|value| *value);
    let _ = append_backchannel_trace(
        request,
        json!({
            "kind":"suggestion_model_timing",
            "unix_ms":unix_ms(),
            "request_id":request.request_id,
            "memo_index":request.memo_index,
            "session_create_ms":session_create_ms,
            "first_text_delta_ms":first_text_delta_ms,
            "completion_ms":completion_ms,
            "system_prompt_chars":system_prompt_chars,
            "prompt_chars":prompt_chars,
            "approx_input_tokens":(system_prompt_chars + prompt_chars).div_ceil(4),
            "output_chars":text.chars().count(),
            "max_output_tokens":BACKCHANNEL_MAX_TOKENS,
        }),
    );
    Ok(extract_json_object(&text).unwrap_or(text))
}

fn handle_backchannel_provider_error(
    request: &BackchannelAiRequest,
    stage: &'static str,
) -> crate::ai_config::ProviderErrorDisposition {
    crate::ai_config::handle_included_ai_provider_error(
        request.ai_credential_generation,
        None,
        stage,
    )
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn build_suggestion_prompt(
    request: &BackchannelAiRequest,
    notes_folder: &str,
    queries: &[String],
    catalyze_context: &str,
    steer_trailer: Option<&str>,
) -> String {
    let instructions = if request.instructions.trim().is_empty() {
        "Act like a calm private noticing partner for high-context live conversations. Notice the latent concern, blind spot, permission gap, or archive-backed pattern that may be present but not yet easy for the user to surface. Use discovery-call instincts only to turn that noticing into one speakable question. Avoid sales coaching, broad diagnosis, advice, and recognition theater.".to_string()
    } else {
        request.instructions.clone()
    };
    let query_list = if queries.is_empty() {
        "(no queries generated)".to_string()
    } else {
        queries
            .iter()
            .map(|query| format!("- {query}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut prompt = format!(
        r#"You are Margins's live backchannel coach. The user may ask for a private cue while in a meeting.

Your persona is spare, directional, and low-drama. The product power you demonstrate is noticing: privately surfacing the latent thing in this moment that the user may not yet have permission, language, or confidence to bring into the room.

You are not here to impress the user with related-evidence recognition. You are here to notice the bridge between the live conversation, the user's memo mark, and the archive context pulled forward below. Then give the user one graceful way to surface it.

Cache-stable context follows. Keep this section before the trigger-specific details so providers that support prompt caching can reuse the large meeting/vault prefix across repeated cues.

Session: {session}
Notes folder: {notes_folder}

User/backchannel instructions:
{instructions}

Interleaved memo/transcript context:
{transcript}

Related evidence results:
{catalyze_context}

Trigger-specific details follow. These fields change for each Cmd+Enter request and should stay at the end of the prompt.

The user explicitly pressed Cmd+Enter on a memo line while in a meeting.

Goal: show exactly one concise private cue if useful. The cue has two visible parts:
- direction: a few words naming what Margins noticed, e.g. "trust under the feature ask", "permission to name proof", "old thread, present opening", "specifics over story", "risk hiding in agreement".
- question: one speakable question or line that helps the user surface the noticing without overreaching.

Ground it in the active memo, the current transcript segment marked in the interleaved context, and the related evidence results. The memo may point to something more salient than the literal current topic; follow that if it better serves the meeting objective. Use previous transcript context only for continuity. It is okay to use relevant private/emotional vault context if it helps the user ask a better question; avoid gratuitous or unrelated private material. Because the user explicitly asked for help, prefer a suggestion over quiet unless the current segment/memo/context are genuinely insufficient.

Return ONLY compact JSON with this schema. Do not add fields:
{{
  "state": "quiet | suggestion_available | missed_opening | follow_up_later | error",
  "type": "permission_gap | blind_spot | unsaid_concern | archive_pattern | unresolved_thread | proof_debt | relationship_obligation | decision_pressure | risk | ask_specifics | clarify_next_step | other",
  "direction": "2-7 words naming what was noticed",
  "suggestion": "one speakable question/line, or empty when quiet",
  "confidence": "low | medium | high"
}}

Hard rules:
- One suggestion max.
- No paragraphs.
- Direction must be compact and specific, not a category label like "Follow-up question" or "Discovery question".
- Suggestion should make the noticed thing speakable in the meeting.
- Use discovery tactics when useful, but make the cue feel like noticing, not interrogation.
- If confidence is low, state must be quiet.
- Do not mention implementation details, audio, ASR, note lookup, search, or tools in the user-facing suggestion.

Memo time: {memo_time}
Active memo: {memo}

Generated search queries:
{queries}
"#,
        session = request.session_name,
        notes_folder = notes_folder,
        memo_time = request.memo_time,
        memo = request.memo_text,
        instructions = instructions,
        transcript = truncate_for_prompt(&request.transcript, 18_000),
        queries = query_list,
        catalyze_context = catalyze_context,
    );

    if let Some(trailer) = steer_trailer.map(str::trim).filter(|s| !s.is_empty()) {
        prompt.push('\n');
        prompt.push_str(trailer);
        prompt.push('\n');
    }

    prompt
}

/// Render the cumulative steer history as an XML block appended after the
/// trigger details. The cache-stable prefix above is untouched, so prompt
/// caching still reuses it; only this tail grows as the user steers.
fn build_steer_trailer(
    original_cue: Option<&str>,
    history: &[SteerExchange],
    pending: &str,
) -> String {
    let mut out = String::new();
    out.push_str(
        "<steering_session note=\"The user is privately steering the cue below. Treat every steer as the user's intent for the cue, not as meeting speech. Apply the steers cumulatively: later steers refine earlier ones, and the pending steer matters most.\">\n",
    );
    if let Some(cue) = original_cue.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "  <original_cue>{}</original_cue>\n",
            xml_escape(cue)
        ));
    }
    for (idx, exchange) in history.iter().enumerate() {
        out.push_str(&format!("  <revision index=\"{}\">\n", idx + 1));
        out.push_str(&format!(
            "    <steer>{}</steer>\n",
            xml_escape(exchange.steering.trim())
        ));
        out.push_str(&format!(
            "    <revised_cue>{}</revised_cue>\n",
            xml_escape(exchange.cue.trim())
        ));
        out.push_str("  </revision>\n");
    }
    out.push_str(&format!(
        "  <pending_steer>{}</pending_steer>\n",
        xml_escape(pending.trim())
    ));
    out.push_str("</steering_session>\n");
    out.push_str(
        "\nRegenerate exactly one cue that satisfies this whole steer history. Honor the pending steer most, keep earlier steers that do not conflict with it, and keep the same grounding, schema, and hard rules.",
    );
    out
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn build_catalyze_input(memo_text: &str, transcript: &str) -> String {
    let recent = recent_words(transcript, 50);
    [memo_text.trim(), recent.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
        .chars()
        .take(700)
        .collect::<String>()
}

/// Catalyze output plus a health signal: `all_results_empty` is true only when
/// every query ran, parsed cleanly, and returned zero results — the fingerprint
/// of an empty/stale enzyme index (a healthy index essentially always returns
/// nearest-neighbor hits).
struct CatalyzeOutcome {
    context: String,
    all_results_empty: bool,
    /// Set when catalyze could not produce any usable context — a spawn error or
    /// a non-zero exit with no parseable results JSON. This is a hard failure,
    /// not an empty index, and must route to a grounded quiet (never the model).
    hard_failure: Option<String>,
}

/// Parse a raw `enzyme catalyze` JSON payload and report whether it has zero
/// results. Returns `None` when the output is not the expected results JSON
/// (e.g. an `{"error":...}` payload or non-JSON), so callers don't mistake a
/// failure for a legitimately empty result set.
fn catalyze_output_empty(raw: &str) -> Option<bool> {
    let value: Value = serde_json::from_str(raw).ok()?;
    if value.get("error").is_some() {
        return None;
    }
    if let Some(total) = value.get("total_results").and_then(Value::as_u64) {
        return Some(total == 0);
    }
    value
        .get("results")
        .and_then(Value::as_array)
        .map(|results| results.is_empty())
}

fn collect_catalyze_results(vault_path: Option<&PathBuf>, queries: &[String]) -> CatalyzeOutcome {
    let Some(vault) = vault_path else {
        return CatalyzeOutcome {
            context: "No notes folder configured; no private context search was run.".to_string(),
            all_results_empty: false,
            hard_failure: None,
        };
    };
    if queries.is_empty() {
        return CatalyzeOutcome {
            context: "No search queries were generated.".to_string(),
            all_results_empty: false,
            hard_failure: None,
        };
    }

    let mut sections = Vec::new();
    let mut parsed = 0usize;
    let mut empty = 0usize;
    let mut first_error: Option<String> = None;
    for query in queries {
        // Enzyme is operated at the PROJECT ROOT: the vault path here is the
        // project root (not the notes/inbox subfolder), and we run the binary
        // with that root as its working directory so it resolves the root-level
        // `.enzyme` context regardless of the host process cwd.
        let output = run_enzyme(
            vec![
                "catalyze".to_string(),
                "--vault".to_string(),
                vault.to_string_lossy().to_string(),
                "--limit".to_string(),
                "2".to_string(),
                query.clone(),
            ],
            Some(vault.as_path()),
        );
        let text = match output {
            Ok(text) if !text.trim().is_empty() => {
                if let Some(is_empty) = catalyze_output_empty(&text) {
                    parsed += 1;
                    if is_empty {
                        empty += 1;
                    }
                }
                compact_catalyze_prompt_context(&text)
            }
            Ok(_) => "(no results)".to_string(),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error.clone());
                }
                format!("(search failed: {error})")
            }
        };
        sections.push(format!("## Query: {query}\n{text}"));
    }
    // Degraded only when EVERY query parsed cleanly and every one was empty —
    // any parse failure or non-empty hit means we can't call the index empty.
    let all_results_empty = parsed == queries.len() && empty == parsed;
    // Hard failure only when NO query yielded parseable results JSON yet at
    // least one errored: catalyze produced no usable context at all, so a model
    // call would be under-grounded. A partial failure alongside real hits is
    // tolerated — we still have context to ground on.
    let hard_failure = if parsed == 0 { first_error } else { None };
    CatalyzeOutcome {
        context: truncate_for_prompt(&sections.join("\n\n"), BACKCHANNEL_CATALYZE_CONTEXT_CHARS),
        all_results_empty,
        hard_failure,
    }
}

fn compact_catalyze_prompt_context(text: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return truncate_for_prompt(text, BACKCHANNEL_CATALYZE_CONTEXT_CHARS);
    };
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return serde_json::to_string(&json!({"error":error,"results":[]}))
            .unwrap_or_else(|_| truncate_for_prompt(text, BACKCHANNEL_CATALYZE_CONTEXT_CHARS));
    }

    let mut out = recall_freshness_stale_prefix(&value);
    if let Some(catalysts) = value
        .get("top_contributing_catalysts")
        .and_then(Value::as_array)
    {
        let lines = catalysts
            .iter()
            .take(4)
            .filter_map(|catalyst| {
                let label = catalyst
                    .get("entity")
                    .or_else(|| catalyst.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("archive signal");
                let text = catalyst
                    .get("text")
                    .or_else(|| catalyst.get("question"))
                    .and_then(Value::as_str)?;
                Some(format!("- {label}: {}", truncate_for_prompt(text, 240)))
            })
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            out.push_str("Archive signals:\n");
            out.push_str(&lines.join("\n"));
            out.push('\n');
        }
    }
    if let Some(results) = value.get("results").and_then(Value::as_array) {
        for result in results.iter().take(2) {
            let heading = recall_result_related_heading(result);
            let content = result
                .get("content")
                .or_else(|| result.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            out.push_str(&format!(
                "{heading}\n{}\n",
                truncate_for_prompt(content, BACKCHANNEL_CATALYZE_RESULT_CHARS)
            ));
        }
    }
    if out.trim().is_empty() {
        truncate_for_prompt(
            &strip_catalyze_prompt_noise(text),
            BACKCHANNEL_CATALYZE_CONTEXT_CHARS,
        )
    } else {
        truncate_for_prompt(&out, BACKCHANNEL_CATALYZE_CONTEXT_CHARS)
    }
}

fn strip_catalyze_prompt_noise(text: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(text) else {
        return text.to_string();
    };
    strip_catalyze_prompt_noise_value(&mut value);
    serde_json::to_string(&value).unwrap_or_else(|_| text.to_string())
}

fn strip_catalyze_prompt_noise_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                strip_catalyze_prompt_noise_value(item);
            }
        }
        Value::Object(map) => {
            map.remove("similarity");
            map.remove("processing_time");
            for value in map.values_mut() {
                strip_catalyze_prompt_noise_value(value);
            }
        }
        _ => {}
    }
}

fn catalyze_question_context(catalyze_context: &str, query: &str) -> String {
    let mut lines = Vec::new();
    if !query.trim().is_empty() {
        lines.push(format!("Query: {}", query.trim()));
    }

    if let Some(json_text) = extract_json_object(catalyze_context) {
        if let Ok(parsed) = serde_json::from_str::<Value>(&json_text) {
            if let Some(catalysts) = parsed.get("top_contributing_catalysts") {
                collect_questionish_values(catalysts, &mut lines);
            }
        }
    }

    if lines.is_empty() {
        "No related-note questions were returned.".to_string()
    } else {
        truncate_for_prompt(&dedupe_lines(lines).join("\n"), 4_000)
    }
}

/// Recognize the "this vault has never been indexed" flavor of catalyze error
/// so the desktop can report an honest unavailable state instead of plain quiet.
/// Recall cannot serve this lookup. Honest terminal state; the mark is still
/// saved and no model is called.
fn local_unavailable_setup(
    request: &BackchannelAiRequest,
    queries: &[String],
    reason: &str,
) -> String {
    serde_json::to_string(&json!({
        "state": "cue_unavailable",
        "type": "other",
        "direction": "",
        "title": "",
        "anchor": format!("{} memo: {}", request.memo_time, request.memo_text),
        "why": "Recall unavailable. Run margins setup.",
        "reason": reason,
        "hint": "run margins setup",
        "suggestion": "",
        "confidence": "low",
        "safety": "Your mark is saved.",
        "sources": [],
        "search_queries": queries,
        "internal_reason": reason,
        "local_only": true,
    }))
    .unwrap_or_else(|_| {
        "{\"state\":\"cue_unavailable\",\"suggestion\":\"\",\"local_only\":true}".to_string()
    })
}

/// True when the cue JSON was produced locally without a model call. Every
/// local fallback tags itself with `local_only` so traces can prove `model_ms`
/// is legitimately zero.
pub(crate) fn raw_is_local_only(raw: &str) -> bool {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|value| value.get("local_only").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn catalyze_unavailable_reason(catalyze_context: &str) -> Option<String> {
    let json_text = extract_json_object(catalyze_context)?;
    let parsed = serde_json::from_str::<Value>(&json_text).ok()?;
    let error = parsed.get("error")?.as_str()?.trim();
    if error.is_empty() {
        return None;
    }
    Some(error.to_string())
}

fn collect_questionish_values(value: &Value, lines: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_questionish_values(item, lines);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                let key_lower = key.to_lowercase();
                if key_lower.contains("question")
                    || key_lower == "query"
                    || key_lower == "name"
                    || key_lower == "text"
                {
                    if let Some(text) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        lines.push(format!(
                            "{key}: {}",
                            text.chars().take(240).collect::<String>()
                        ));
                    }
                } else if key_lower.contains("catalyst") {
                    collect_questionish_values(value, lines);
                }
            }
        }
        _ => {}
    }
}

fn dedupe_lines(lines: Vec<String>) -> Vec<String> {
    lines.into_iter().fold(Vec::new(), |mut acc, line| {
        if !acc
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&line))
        {
            acc.push(line);
        }
        acc
    })
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

pub(crate) fn run_enzyme_pub(
    args: Vec<String>,
    work_dir: Option<&std::path::Path>,
) -> Result<String, String> {
    run_enzyme(args, work_dir)
}

// In-process petri/catalyze via the vendored recall engine (no subprocess).
fn run_enzyme(args: Vec<String>, _work_dir: Option<&std::path::Path>) -> Result<String, String> {
    crate::recall_search_compat(&args)
}

fn truncate_for_prompt(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push_str("\n…(truncated)");
    out
}

fn recent_words(text: &str, max_words: usize) -> String {
    if max_words == 0 {
        return String::new();
    }
    let cleaned = text
        .replace('<', " ")
        .replace('>', " ")
        .replace('/', " ")
        .replace('"', " ");
    let words = cleaned.split_whitespace().collect::<Vec<_>>();
    let start = words.len().saturating_sub(max_words);
    words[start..].join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalyze_input_uses_memo_and_recent_transcript_tail() {
        let transcript = (0..80)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let input = build_catalyze_input("pricing concern", &transcript);

        assert!(input.starts_with("pricing concern"));
        assert!(input.contains("word30"));
        assert!(input.contains("word79"));
        assert!(!input.contains("word0 "));
    }

    #[test]
    fn catalyze_output_empty_distinguishes_empty_hits_and_errors() {
        // Empty index / no hits — the degraded fingerprint.
        assert_eq!(
            catalyze_output_empty(
                r#"{"results":[],"total_results":0,"top_contributing_catalysts":[]}"#
            ),
            Some(true)
        );
        // Real hits present.
        assert_eq!(
            catalyze_output_empty(r#"{"results":[{"content":"note"}],"total_results":2}"#),
            Some(false)
        );
        // Empty `results` with no total_results field still counts as empty.
        assert_eq!(catalyze_output_empty(r#"{"results":[]}"#), Some(true));
        // An error payload must NOT read as "empty" — it's a failure, handled elsewhere.
        assert_eq!(
            catalyze_output_empty(r#"{"error":"Vault not initialized","results":[]}"#),
            None
        );
        // Non-JSON / unexpected output is inconclusive, not empty.
        assert_eq!(catalyze_output_empty("(search failed: boom)"), None);
    }

    #[test]
    fn catalyze_question_context_ignores_result_content() {
        let raw = r#"{"query":"x","results":[{"content":"secret note body"}],"top_contributing_catalysts":[{"name":"Pilot risk","question":"What changed in the rollout?"}]}"#;
        let context = catalyze_question_context(raw, "memo query");

        assert!(context.contains("memo query"));
        assert!(context.contains("Pilot risk"));
        assert!(context.contains("What changed"));
        assert!(!context.contains("secret note body"));
    }

    #[test]
    fn catalyze_question_context_extracts_catalyst_text_field() {
        let raw = r#"{"top_contributing_catalysts":[{"entity":"Pilot risk","text":"What proof would make this rollout feel safe?","relevance_score":0.82}],"results":[{"text":"note result text"}]}"#;
        let context = catalyze_question_context(raw, "memo query");

        assert!(context.contains("What proof would make this rollout feel safe?"));
        assert!(!context.contains("note result text"));
    }

    #[test]
    fn strip_catalyze_prompt_noise_removes_similarity_and_processing_time() {
        let raw = r#"{"processing_time":0.12,"results":[{"document_ref":"a.md","evidence":{"kind":"native_markdown","path":"/vault/a.md"},"content":"note body","similarity":0.82},{"document_ref":"b.md","evidence":{"kind":"native_markdown","path":"/vault/b.md"},"content":"second","metadata":{"processing_time":1.7,"similarity":0.44}}]}"#;
        let stripped = strip_catalyze_prompt_noise(raw);

        assert!(stripped.contains(r#""path":"/vault/a.md""#));
        assert!(stripped.contains(r#""content":"note body""#));
        assert!(stripped.contains(r#""path":"/vault/b.md""#));
        assert!(!stripped.contains("similarity"));
        assert!(!stripped.contains("processing_time"));
    }

    #[test]
    fn compact_catalyze_context_keeps_bounded_signals_and_note_excerpts() {
        let long_body = "detail ".repeat(300);
        let raw = serde_json::to_string(&json!({
            "schema_version": "margins.recall.v1",
            "status": "ok",
            "processing_time": 1.2,
            "top_contributing_catalysts": [
                {"entity":"Pilot risk","text":"What proof would make this rollout feel safe?","similarity":0.91}
            ],
            "results": [
                {"document_ref":"meetings/pilot.md","source_kind":"notes","evidence":{"kind":"native_markdown","path":"meetings/pilot.md"},"content":long_body,"similarity":0.82},
                {"document_ref":"people/alex.md","source_kind":"notes","evidence":{"kind":"native_markdown","path":"people/alex.md"},"content":"Alex needs concrete evidence before rollout."},
                {"document_ref":"ignored.md","source_kind":"notes","evidence":{"kind":"native_markdown","path":"ignored.md"},"content":"third result must not enter the prompt"}
            ]
        }))
        .unwrap();

        let compact = compact_catalyze_prompt_context(&raw);

        assert!(compact.contains("Pilot risk"));
        assert!(compact.contains("Related note: pilot"));
        assert!(compact.contains("Related note: alex"));
        assert!(!compact.contains("ignored.md"));
        assert!(!compact.contains("similarity"));
        assert!(!compact.contains("processing_time"));
        assert!(compact.chars().count() <= BACKCHANNEL_CATALYZE_CONTEXT_CHARS + 20);
    }

    #[test]
    fn compact_catalyze_context_preserves_stale_freshness_signal() {
        let raw = serde_json::to_string(&json!({
            "schema_version": "margins.recall.v1",
            "status": "ok",
            "freshness": {"status":"stale","stale":true,"reason":"index behind ledger","index":{"status":"stale","stale":true,"reason":"index behind ledger"},"materialization":[]},
            "results": [
                {"document_ref":"meetings/pilot.md","source_kind":"notes","evidence":{"kind":"native_markdown","path":"meetings/pilot.md"},"content":"still usable hit"}
            ]
        }))
        .unwrap();

        let compact = compact_catalyze_prompt_context(&raw);

        assert!(compact.contains("[stale evidence: index behind ledger]"));
        assert!(compact.contains("Related note: pilot"));
        assert!(compact.contains("still usable hit"));
    }

    #[test]
    fn compact_catalyze_context_labels_external_records_without_note_claims() {
        let raw = serde_json::to_string(&json!({
            "schema_version": "margins.recall.v1",
            "status": "ok",
            "results": [
                {"document_ref":"mail:thread-9","source_kind":"google-mail","evidence":{"kind":"external_record","connector_id":"email","source_account":"owner@example.com","source_id":"thread-9"},"content":"prior thread about rollout"}
            ]
        }))
        .unwrap();

        let compact = compact_catalyze_prompt_context(&raw);

        assert!(compact.contains("email thread thread-9"));
        assert!(!compact.contains("Related note"));
        assert!(compact.contains("prior thread about rollout"));
    }

    #[test]
    fn compact_catalyze_context_preserves_errors_for_the_unavailable_gate() {
        let compact = compact_catalyze_prompt_context(
            r#"{"error":"Vault not initialized","results":[{"content":"ignore"}]}"#,
        );

        assert!(compact.contains("Vault not initialized"));
        assert!(!compact.contains("ignore"));
        assert_eq!(
            catalyze_unavailable_reason(&format!("## Query: x\n{compact}")).as_deref(),
            Some("Vault not initialized")
        );
    }

    #[test]
    fn detects_catalyze_error_context() {
        let context = r#"## Query: hello
{"error":"Vault not initialized. Run 'enzyme init' first.","results":[]}"#;

        assert_eq!(
            catalyze_unavailable_reason(context).as_deref(),
            Some("Vault not initialized. Run 'enzyme init' first.")
        );
    }

    #[test]
    fn local_unavailable_result_keeps_failed_catalyze_off_model_path() {
        let request = BackchannelAiRequest {
            request_id: "session:0".to_string(),
            memo_index: 0,
            work_dir: PathBuf::from("/tmp/work"),
            margins_dir: PathBuf::from("/tmp/work/.margins"),
            session_name: "session".to_string(),
            memo_text: "active memo".to_string(),
            memo_time: "10:28".to_string(),
            transcript: "transcript".to_string(),
            catalyze_transcript: "transcript".to_string(),
            vault_path: Some(PathBuf::from("/tmp/work")),
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: None,
            instructions: String::new(),
        };
        let raw = local_unavailable_setup(
            &request,
            &["active memo\n\ntranscript".to_string()],
            "Vault not initialized. Run 'enzyme init' first.",
        );
        let parsed: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(parsed["state"], "cue_unavailable");
        assert_eq!(parsed["confidence"], "low");
        assert!(parsed["suggestion"].as_str().unwrap().is_empty());
        assert_eq!(
            parsed["internal_reason"],
            "Vault not initialized. Run 'enzyme init' first."
        );
    }

    fn request_fixture() -> BackchannelAiRequest {
        BackchannelAiRequest {
            request_id: "session:0:0".to_string(),
            memo_index: 0,
            work_dir: PathBuf::from("/tmp/work"),
            margins_dir: PathBuf::from("/tmp/work/.margins"),
            session_name: "session".to_string(),
            memo_text: "active memo".to_string(),
            memo_time: "10:28".to_string(),
            transcript: "transcript".to_string(),
            catalyze_transcript: "transcript".to_string(),
            vault_path: Some(PathBuf::from("/tmp/work")),
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: None,
            instructions: String::new(),
        }
    }

    fn context_with_error(reason: &str) -> BackchannelContext {
        BackchannelContext {
            queries: vec!["active memo\n\ntranscript".to_string()],
            catalyze_context: format!("{{\"error\":\"{reason}\",\"results\":[]}}"),
            catalyze_error: Some(reason.to_string()),
            notes_folder: "/tmp/work".to_string(),
        }
    }

    #[test]
    fn uninitialized_vault_routes_to_unavailable_not_quiet() {
        let request = request_fixture();
        let context = context_with_error("Vault not initialized. Run 'enzyme init' first.");
        let raw = run_backchannel_ai(request, &context).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(parsed["state"], "cue_unavailable");
        assert!(parsed["suggestion"].as_str().unwrap().is_empty());
        // Every local outcome is provably no-model.
        assert_eq!(parsed["local_only"], true);
        assert!(raw_is_local_only(&raw));
    }

    #[test]
    fn other_catalyze_errors_are_also_explicitly_unavailable() {
        let request = request_fixture();
        let context = context_with_error("recall lookup failed: connection refused");
        let raw = run_backchannel_ai(request, &context).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(parsed["state"], "cue_unavailable");
        assert_eq!(parsed["hint"], "run margins setup");
        assert!(parsed["suggestion"].as_str().unwrap().is_empty());
        assert!(raw_is_local_only(&raw));
    }

    #[test]
    fn hard_process_failure_is_treated_as_catalyze_unavailable() {
        // A spawn error / non-zero exit with no parseable results JSON must be a
        // hard failure so it routes to unavailable, never a bare-context model call.
        let outcome = CatalyzeOutcome {
            context: "## Query: x\n(search failed: recall service not found)".to_string(),
            all_results_empty: false,
            hard_failure: Some("failed to run recall service: No such file".to_string()),
        };
        assert!(outcome.hard_failure.is_some());
    }

    #[test]
    fn raw_is_local_only_is_false_for_model_shaped_json() {
        // A model-produced cue has no `local_only` tag, so model_ms stays real.
        let model_cue = r#"{"state":"suggestion_available","suggestion":"what changed?"}"#;
        assert!(!raw_is_local_only(model_cue));
        assert!(!raw_is_local_only("not json at all"));
    }

    #[test]
    fn suggestion_prompt_keeps_large_context_before_trigger_details() {
        let request = BackchannelAiRequest {
            request_id: "session:0".to_string(),
            memo_index: 0,
            work_dir: PathBuf::from("/tmp/work"),
            margins_dir: PathBuf::from("/tmp/work/.margins"),
            session_name: "session".to_string(),
            memo_text: "active memo".to_string(),
            memo_time: "10:28".to_string(),
            transcript: "stable transcript context".to_string(),
            catalyze_transcript: String::new(),
            vault_path: None,
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: None,
            instructions: "custom instructions".to_string(),
        };
        let prompt = build_suggestion_prompt(
            &request,
            "/vault",
            &["dynamic query".to_string()],
            "stable related-note context",
            None,
        );

        let transcript_at = prompt.find("stable transcript context").unwrap();
        let catalyze_at = prompt.find("stable related-note context").unwrap();
        let trigger_at = prompt
            .find("The user explicitly pressed Cmd+Enter")
            .unwrap();
        let memo_at = prompt.find("Active memo: active memo").unwrap();
        let query_at = prompt.find("dynamic query").unwrap();

        assert!(transcript_at < trigger_at);
        assert!(catalyze_at < trigger_at);
        assert!(trigger_at < memo_at);
        assert!(memo_at < query_at);
    }

    #[test]
    fn steer_trailer_stays_after_the_cache_stable_prefix() {
        let request = BackchannelAiRequest {
            request_id: "session:0".to_string(),
            memo_index: 0,
            work_dir: PathBuf::from("/tmp/work"),
            margins_dir: PathBuf::from("/tmp/work/.margins"),
            session_name: "session".to_string(),
            memo_text: "active memo".to_string(),
            memo_time: "10:28".to_string(),
            transcript: "stable transcript context".to_string(),
            catalyze_transcript: String::new(),
            vault_path: None,
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: None,
            instructions: "custom instructions".to_string(),
        };
        let base = build_suggestion_prompt(
            &request,
            "/vault",
            &["dynamic query".to_string()],
            "stable related-note context",
            None,
        );
        let history = [SteerExchange {
            steering: "less salesy".to_string(),
            cue: "What's blocking the rollout?".to_string(),
        }];
        let trailer = build_steer_trailer(
            Some("Should we frame this as infrastructure?"),
            &history,
            "ask about their timeline",
        );
        let steered = build_suggestion_prompt(
            &request,
            "/vault",
            &["dynamic query".to_string()],
            "stable related-note context",
            Some(trailer.as_str()),
        );

        // The steered prompt extends the base verbatim — identical cache prefix.
        assert!(steered.starts_with(&base));
        let query_at = steered.find("dynamic query").unwrap();
        let session_at = steered.find("<steering_session").unwrap();
        assert!(query_at < session_at);
        // Cumulative lineage: original cue, the prior revision, and the pending steer.
        assert!(steered
            .contains("<original_cue>Should we frame this as infrastructure?</original_cue>"));
        assert!(steered.contains("<revision index=\"1\">"));
        assert!(steered.contains("<steer>less salesy</steer>"));
        assert!(steered.contains("<revised_cue>What's blocking the rollout?</revised_cue>"));
        assert!(steered.contains("<pending_steer>ask about their timeline</pending_steer>"));
    }

    #[test]
    fn steer_trailer_escapes_xml_and_orders_revisions() {
        let history = [
            SteerExchange {
                steering: "first".to_string(),
                cue: "cue one".to_string(),
            },
            SteerExchange {
                steering: "tie A & B <together>".to_string(),
                cue: "cue two".to_string(),
            },
        ];
        let trailer = build_steer_trailer(Some("orig"), &history, "make it < shorter");

        assert!(trailer.contains("tie A &amp; B &lt;together&gt;"));
        assert!(trailer.contains("<pending_steer>make it &lt; shorter</pending_steer>"));
        let first = trailer.find("index=\"1\"").unwrap();
        let second = trailer.find("index=\"2\"").unwrap();
        assert!(first < second);
    }

    #[test]
    fn backchannel_provider_failure_invalidates_propagated_generation() {
        let generation = crate::ai_config::install_test_included_lease_generation();
        let request = BackchannelAiRequest {
            request_id: "generation-test".to_string(),
            memo_index: 0,
            work_dir: PathBuf::from("/tmp/work"),
            margins_dir: PathBuf::from("/tmp/work/.margins"),
            session_name: "session".to_string(),
            memo_text: String::new(),
            memo_time: "00:00".to_string(),
            transcript: String::new(),
            catalyze_transcript: String::new(),
            vault_path: None,
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: Some(generation),
            instructions: String::new(),
        };
        assert!(crate::ai_config::test_included_lease_generation_is_usable(
            generation
        ));
        assert_eq!(
            handle_backchannel_provider_error(&request, "backchannel_prompt"),
            crate::ai_config::ProviderErrorDisposition::Invalidated
        );
        assert!(!crate::ai_config::test_included_lease_generation_is_usable(
            generation
        ));
    }
}
