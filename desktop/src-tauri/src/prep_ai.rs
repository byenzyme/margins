/// Prep-sketch hydration — static dossier + one-shot LLM marginalia.
///
/// Architecture mirrors `backchannel_ai.rs`:
/// - `PrepAiRequest`  — plain data, no app-state references
/// - `PrepContext`    — frozen once per hydration, reused for steering
/// - `PrepSnapshot`   — stored in AppState keyed by session name
/// - `run_prep_hydration` / `run_prep_steer` — blocking entry points
use pi::sdk::{create_agent_session_with_store, SessionOptions, SessionStoreKind, ThinkingLevel};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::pi_events::{
    assistant_text, note_text_delta, recall_freshness_stale_prefix, recall_result_related_heading,
};

// ---------------------------------------------------------------------------
// Token / char budgets
// ---------------------------------------------------------------------------

const PREP_MAX_TOKENS: u32 = 1536;
const PREP_READER_MAX_TOKENS: u32 = 256;
const PREP_DOSSIER_BUDGET_CHARS: usize = 10_000;
const PREP_CATALYZE_RESULT_CHARS: usize = 600;
const PREP_CATALYZE_CONTEXT_CHARS: usize = 3_000;

// ---------------------------------------------------------------------------
// System prompt — persona verbatim from design doc + exemplar pairs
// ---------------------------------------------------------------------------

const PREP_SYSTEM_PROMPT: &str = "You are the prep partner inside Margins \u{2014} the colleague who was in every one of the user\u{2019}s past meetings and is now reading their prep sketch over their shoulder, a few minutes before they walk in. You hold the archive so the user can hold the room.

The sketch is a draft of the user\u{2019}s intent, not a query. Read it the way a trusted chief of staff reads a draft agenda, and meet it at its own depth. When it is a skeleton \u{2014} three bullets dashed off \u{2014} your job is memory: bring back the threads still open with these people, the promises unkept in either direction, the thing they raised last time that the sketch doesn\u{2019}t mention. When it is thought through \u{2014} the user is working out what they believe, what they\u{2019}re worried about, what they want from the room \u{2014} your job is engagement: sharpen the question they are really asking, name the assumption doing the most work, and bring the receipt when the archive supports or contradicts them.

You annotate; you never replace. Every line you write attaches to something: a specific line of the sketch, or a gap the sketch leaves open. Write in the margin\u{2019}s register \u{2014} spare, concrete, in the user\u{2019}s own vocabulary where the sketch supplies it. A half-line with a receipt beats a paragraph of synthesis.

You only assert the past with receipts. Claims about what happened, what was promised, or what was said come exclusively from the dossier and search results below, and carry their source. If the archive is thin, say less; an empty margin is honest, a confident guess is poison. Never invent a carried item, never pad to fill a shape, and never restate the sketch back at the user.

Never coach the user to win, corner, or leverage private information against the people in the room. Prep is for presence, not ambush.

The unprompted utterance register: items must be question-shaped with a receipt (“Did the pricing thread with Sam close? Your 7/12 note leaves it open”). Questions degrade gracefully; assertions degrade catastrophically. Cap at 2–3 items in the typical case, 5 maximum. Silence is what makes speech credible — quiet is the common case.

Each marginalia text is cue-sized: at most two short sentences, roughly 35 words. The receipt is the source citation, not a retelling — never unpack the dossier into the text. If an item needs a paragraph, it is two items or it is not ready.

Good examples:
- “pricing — last time Sam pushed back on per-seat; you parked it [2026-07-12 · 3 wks]”  (skeleton annotation, carries receipt)
- “your worry here assumes churn is price-driven — your 6/30 note has two customers saying onboarding, not price”  (elaborate engagement, anchored, receipt-backed)

Bad examples (never produce):
- “Consider asking about their goals.”  (generic, no receipt, no anchor)
- Four-sentence paragraph synthesizing the dossier back at the user.

Do not write files. Return only strict JSON matching the requested schema.";

// ---------------------------------------------------------------------------
// Request & context structs
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct PrepAiRequest {
    pub(crate) request_id: String,
    pub(crate) work_dir: PathBuf,
    pub(crate) margins_dir: PathBuf,
    pub(crate) session_name: String,
    /// All sketch lines joined verbatim with newlines — never parsed, never summarized.
    pub(crate) sketch_text: String,
    pub(crate) people: Vec<String>,
    pub(crate) event_title: Option<String>,
    pub(crate) vault_path: Option<PathBuf>,
    pub(crate) people_folder: String,
    pub(crate) ai_provider: Option<String>,
    pub(crate) ai_model: Option<String>,
    pub(crate) ai_api_key: Option<String>,
    pub(crate) ai_credential_generation: Option<u64>,
    /// Same settings slot as backchannel instructions.
    pub(crate) instructions: String,
    /// Which clock-stopped block this hydration is for (0 = prep, N = pause after segment N).
    pub(crate) block_ordinal: u32,
    /// Verbatim texts already pulled into the body this block; model must not re-offer them.
    pub(crate) pulled_texts: Vec<String>,
    /// Positioned prior-blocks/timeline context for mid-meeting pauses (block_ordinal > 0).
    pub(crate) meeting_so_far: Option<String>,
}

/// Composite snapshot key: `"session_name#block_ordinal"`.
pub(crate) fn prep_snapshot_key(session: &str, block: u32) -> String {
    format!("{session}#{block}")
}

/// Frozen context built once per hydration; reused for steering (cache-stable prefix).
#[derive(Clone)]
pub(crate) struct PrepContext {
    /// Reader-pass queries derived from sketch content.
    pub(crate) queries: Vec<String>,
    /// Deterministic dossier: person notes + prior-session fragments, tagged with source.
    pub(crate) dossier: String,
    /// Semantic layer: enzyme catalyze results for reader queries.
    pub(crate) catalyze_context: String,
    pub(crate) catalyze_error: Option<String>,
    pub(crate) catalyze_degraded: bool,
}

/// One steering round.
#[derive(Clone)]
pub(crate) struct SteerExchange {
    pub(crate) instruction: String,
    pub(crate) result: String,
}

/// Snapshot stored in AppState, keyed by session_name.
#[derive(Clone)]
pub(crate) struct PrepSnapshot {
    pub(crate) session_name: String,
    pub(crate) request: PrepAiRequest,
    pub(crate) context: PrepContext,
    pub(crate) original_marginalia: Option<String>,
    pub(crate) steer_history: Vec<SteerExchange>,
}

// ---------------------------------------------------------------------------
// Output types — post-validated before emission
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Marginalia {
    pub(crate) anchor: Option<String>,
    pub(crate) kind: String,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) sources: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct HydrationResult {
    pub(crate) state: String,
    pub(crate) posture: Option<String>,
    pub(crate) marginalia: Vec<Marginalia>,
    #[serde(default)]
    pub(crate) reason: Option<String>,
    #[serde(default)]
    pub(crate) hint: Option<String>,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

pub(crate) fn run_prep_hydration(request: PrepAiRequest) -> Result<HydrationResult, String> {
    let context = build_prep_context(&request);
    run_prep_hydration_with_context(&request, &context, None, &[])
}

/// Public variant used by lib.rs when context is pre-built to avoid double-building.
pub(crate) fn run_prep_hydration_with_context_pub(
    request: &PrepAiRequest,
    context: &PrepContext,
) -> Result<HydrationResult, String> {
    run_prep_hydration_with_context(request, context, None, &request.pulled_texts.clone())
}

pub(crate) fn run_prep_steer(
    request: PrepAiRequest,
    context: &PrepContext,
    original_marginalia: Option<&str>,
    history: &[SteerExchange],
    instruction: String,
) -> Result<HydrationResult, String> {
    let pulled = request.pulled_texts.clone();
    let trailer = build_steer_trailer(original_marginalia, history, &instruction);
    run_prep_hydration_with_context(&request, context, Some(trailer.as_str()), &pulled)
}

/// Build the frozen PrepContext (reader pass + dossier + catalyze).
pub(crate) fn build_prep_context(request: &PrepAiRequest) -> PrepContext {
    // --- Reader pass (Call 0): cheap LLM to get catalyze queries + entities ---
    let (queries, _entities) = reader_pass(request);

    let _ = append_prep_trace(
        request,
        json!({
            "kind": "reader_pass",
            "queries": queries,
            "sketch_chars": request.sketch_text.chars().count(),
        }),
    );

    // --- Deterministic dossier (no model) ---
    let dossier = build_dossier(request);

    let _ = append_prep_trace(
        request,
        json!({
            "kind": "dossier_built",
            "dossier_chars": dossier.chars().count(),
        }),
    );

    // --- Semantic layer (enzyme catalyze) ---
    let catalyze = collect_catalyze_results(request.vault_path.as_ref(), &queries);
    let catalyze_error =
        catalyze_unavailable_reason(&catalyze.context).or_else(|| catalyze.hard_failure.clone());
    let catalyze_degraded = catalyze_error.is_none() && catalyze.all_results_empty;

    let _ = append_prep_trace(
        request,
        json!({
            "kind": "related_note_context",
            "queries": queries,
            "error": catalyze_error,
            "degraded": catalyze_degraded,
        }),
    );

    PrepContext {
        queries,
        dossier,
        catalyze_context: catalyze.context,
        catalyze_error,
        catalyze_degraded,
    }
}

// ---------------------------------------------------------------------------
// Reader pass (Call 0) — cheap LLM to derive catalyze queries and entities
// ---------------------------------------------------------------------------

fn reader_pass(request: &PrepAiRequest) -> (Vec<String>, Vec<String>) {
    // Fallback: mechanical concat from sketch lines + people names (like build_catalyze_input).
    let fallback_queries = build_fallback_queries(request);

    if request.ai_provider.is_none() && request.ai_api_key.is_none() {
        return (fallback_queries, vec![]);
    }

    let system = "You are a prep-pass reader. Given a meeting prep sketch and attendee list, extract:\n1. queries[]: 2-5 search queries phrased from what the sketch is ABOUT (not its literal lines). Each query should surface relevant notes from a personal vault.\n2. entities[]: person or project names mentioned in prose that are NOT in the attendee list.\n\nReturn ONLY strict JSON: {\"queries\": [\"...\"], \"entities\": [\"...\"]}";

    let attendees = if request.people.is_empty() {
        "(none listed)".to_string()
    } else {
        request.people.join(", ")
    };
    let event_line = request
        .event_title
        .as_deref()
        .map(|t| format!("Event: {t}\n"))
        .unwrap_or_default();
    let prompt = format!(
        "{event_line}Attendees: {attendees}\n\nSketch:\n{sketch}",
        sketch = request.sketch_text
    );

    let result =
        prompt_prep_model_blocking(request, system.to_string(), prompt, PREP_READER_MAX_TOKENS);
    match result {
        Ok(raw) => {
            if let Some(json_str) = extract_json_object(&raw) {
                if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
                    let queries = parsed
                        .get("queries")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .take(5)
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let entities = parsed
                        .get("entities")
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if !queries.is_empty() {
                        return (queries, entities);
                    }
                }
            }
            (fallback_queries, vec![])
        }
        Err(_) => (fallback_queries, vec![]),
    }
}

fn build_fallback_queries(request: &PrepAiRequest) -> Vec<String> {
    // Emit MULTIPLE distinct queries (mirroring the reader LLM's 2-5 query
    // decomposition), not one joined blob. When the reader pass fails on a
    // re-fire and we fall back here, a single concatenated query retrieves
    // poorly from the vault — it finds nothing, the model reports "no notes",
    // and that empty result wholesale-supersedes a good first pass. Separate
    // angles (each sketch line, attendees, title) keep retrieval usable.
    let mut queries: Vec<String> = vec![];
    // Each non-empty sketch line is its own thought → its own query.
    for line in request.sketch_text.lines() {
        let q = line.trim();
        if !q.is_empty() {
            queries.push(q.chars().take(200).collect::<String>());
        }
    }
    // Attendees as a retrieval angle (prior notes with these people).
    if !request.people.is_empty() {
        queries.push(request.people.join(" "));
    }
    // Event title as a topical angle.
    if let Some(title) = &request.event_title {
        let t = title.trim();
        if !t.is_empty() {
            queries.push(t.to_string());
        }
    }
    // Dedup case-insensitively and cap at 5 (matches the reader-pass ceiling).
    let mut seen = std::collections::HashSet::new();
    queries.retain(|q| seen.insert(q.to_lowercase()));
    queries.truncate(5);
    queries
}

// ---------------------------------------------------------------------------
// Deterministic dossier (no LLM)
// ---------------------------------------------------------------------------

fn build_dossier(request: &PrepAiRequest) -> String {
    let Some(vault) = &request.vault_path else {
        return "(no vault configured; no dossier)".to_string();
    };

    let mut sections: Vec<String> = vec![];

    // Person notes for each attendee
    for person in &request.people {
        let note_path = vault
            .join(&request.people_folder)
            .join(format!("{person}.md"));
        if let Ok(content) = std::fs::read_to_string(&note_path) {
            let trimmed = truncate_for_dossier(&content, 2_000);
            let vault_rel = note_path
                .strip_prefix(vault)
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| note_path.display().to_string());
            sections.push(format!(
                "### Person note: {person}\n[source: {vault_rel}]\n{trimmed}"
            ));
        }
    }

    // Prior sessions with people overlap — up to 3 per person
    let sessions = margins::session_index::list_sessions_with_notes(&request.work_dir, None, None)
        .unwrap_or_default();

    let people_lower: Vec<String> = request.people.iter().map(|p| p.to_lowercase()).collect();

    // Filter to sessions with at least one attendee overlap, newest first
    let mut matching: Vec<&margins::session_index::SessionInfoDto> = sessions
        .iter()
        .filter(|s| {
            if people_lower.is_empty() {
                return false;
            }
            let session_people = s.display_people();
            session_people.iter().any(|p| {
                let pl = p.to_lowercase();
                people_lower
                    .iter()
                    .any(|target| pl.contains(target.as_str()) || target.contains(pl.as_str()))
            })
        })
        .collect();

    // sort newest first by name (names are typically date-prefixed)
    matching.sort_by(|a, b| b.name.cmp(&a.name));
    matching.truncate(3);

    for session in matching {
        let Some(note_path_str) = &session.vault_note_path else {
            continue;
        };
        let note_path = if std::path::Path::new(note_path_str).is_absolute() {
            std::path::PathBuf::from(note_path_str)
        } else {
            vault.join(note_path_str)
        };
        let Ok(content) = std::fs::read_to_string(&note_path) else {
            continue;
        };

        let vault_rel = note_path
            .strip_prefix(vault)
            .ok()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| note_path_str.clone());

        let date_label = &session.name;
        let age = session_age_label(&session.name);
        let source_tag = format!("[source: {vault_rel} \u{00b7} {date_label} \u{00b7} {age}]");

        let fragments = extract_open_fragments(&content, &source_tag);
        if !fragments.is_empty() {
            sections.push(format!(
                "### Prior session: {}\n{}",
                session.name,
                fragments.join("\n")
            ));
        }
    }

    if sections.is_empty() {
        return "(no prior context found for these attendees)".to_string();
    }

    let full = sections.join("\n\n");
    truncate_for_dossier(&full, PREP_DOSSIER_BUDGET_CHARS)
}

/// Extract unchecked action items and open-question sections from a note.
pub(crate) fn extract_open_fragments(content: &str, source_tag: &str) -> Vec<String> {
    let mut fragments = vec![];

    let open_section_keywords = [
        "follow-up",
        "follow up",
        "open question",
        "unresolved",
        "carry",
        "carries",
    ];

    let mut in_open_section = false;
    let mut current_section_items: Vec<String> = vec![];

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect heading
        if trimmed.starts_with('#') {
            // Flush previous open section
            if in_open_section && !current_section_items.is_empty() {
                for item in current_section_items.drain(..) {
                    fragments.push(format!("{item}\n  {source_tag}"));
                }
            }
            let heading_lower = trimmed.to_lowercase();
            in_open_section = open_section_keywords
                .iter()
                .any(|kw| heading_lower.contains(kw));
            continue;
        }

        // Unchecked checkbox anywhere in note
        if trimmed.starts_with("- [ ]") {
            let item_text = trimmed.trim_start_matches("- [ ]").trim();
            if !item_text.is_empty() {
                fragments.push(format!("- [ ] {item_text}\n  {source_tag}"));
            }
            continue;
        }

        // Lines in an open section (non-heading, non-empty)
        if in_open_section && !trimmed.is_empty() && !trimmed.starts_with("- [x]") {
            current_section_items.push(format!("  {trimmed}"));
        }
    }

    // Flush last section
    if in_open_section && !current_section_items.is_empty() {
        for item in current_section_items {
            fragments.push(format!("{item}\n  {source_tag}"));
        }
    }

    fragments
}

fn session_age_label(session_name: &str) -> String {
    // Session names are often date-prefixed like "2026-07-12-..." or "20260712-..."
    // Collect first 8 digits by stripping non-digit separators.
    let digits: String = session_name
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(8)
        .collect();
    if digits.len() == 8 {
        if let (Ok(y), Ok(m), Ok(d)) = (
            digits[0..4].parse::<i32>(),
            digits[4..6].parse::<u32>(),
            digits[6..8].parse::<u32>(),
        ) {
            if let Some(date) = chrono::NaiveDate::from_ymd_opt(y, m, d) {
                let today = chrono::Local::now().date_naive();
                let days = (today - date).num_days();
                return if days == 0 {
                    "today".to_string()
                } else if days == 1 {
                    "yesterday".to_string()
                } else if days < 7 {
                    format!("{days}d")
                } else if days < 30 {
                    format!("{} wk{}", days / 7, if days / 7 == 1 { "" } else { "s" })
                } else if days < 365 {
                    format!("{} mo", days / 30)
                } else {
                    format!("{} yr", days / 365)
                };
            }
        }
    }
    "".to_string()
}

fn truncate_for_dossier(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out = text.chars().take(max).collect::<String>();
    out.push_str("\n\u{2026}(truncated)");
    out
}

// ---------------------------------------------------------------------------
// Semantic layer (enzyme catalyze) — reuses backchannel patterns
// ---------------------------------------------------------------------------

struct CatalyzeOutcome {
    context: String,
    all_results_empty: bool,
    hard_failure: Option<String>,
}

fn collect_catalyze_results(vault_path: Option<&PathBuf>, queries: &[String]) -> CatalyzeOutcome {
    let Some(vault) = vault_path else {
        return CatalyzeOutcome {
            context: "No vault configured.".to_string(),
            all_results_empty: false,
            hard_failure: None,
        };
    };
    if queries.is_empty() {
        return CatalyzeOutcome {
            context: "No queries generated.".to_string(),
            all_results_empty: false,
            hard_failure: None,
        };
    }

    let mut sections = vec![];
    let mut parsed = 0usize;
    let mut empty = 0usize;
    let mut first_error: Option<String> = None;

    for query in queries {
        let output = crate::backchannel_ai::run_enzyme_pub(
            vec![
                "catalyze".to_string(),
                "--vault".to_string(),
                vault.to_string_lossy().to_string(),
                "--limit".to_string(),
                "4".to_string(),
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
                compact_catalyze_context(
                    &text,
                    PREP_CATALYZE_RESULT_CHARS,
                    PREP_CATALYZE_CONTEXT_CHARS,
                )
            }
            Ok(_) => "(no results)".to_string(),
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err.clone());
                }
                format!("(search failed: {err})")
            }
        };
        sections.push(format!("## Query: {query}\n{text}"));
    }

    let all_results_empty = parsed == queries.len() && empty == parsed;
    let hard_failure = if parsed == 0 { first_error } else { None };

    CatalyzeOutcome {
        context: truncate_for_dossier(&sections.join("\n\n"), PREP_CATALYZE_CONTEXT_CHARS),
        all_results_empty,
        hard_failure,
    }
}

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
        .map(|r| r.is_empty())
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

fn compact_catalyze_context(text: &str, result_chars: usize, context_chars: usize) -> String {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return truncate_for_dossier(text, context_chars);
    };
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return serde_json::to_string(&json!({"error": error, "results": []}))
            .unwrap_or_else(|_| truncate_for_dossier(text, context_chars));
    }

    let mut out = recall_freshness_stale_prefix(&value);
    if let Some(catalysts) = value
        .get("top_contributing_catalysts")
        .and_then(Value::as_array)
    {
        let lines = catalysts
            .iter()
            .take(4)
            .filter_map(|c| {
                let label = c
                    .get("entity")
                    .or_else(|| c.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("archive signal");
                let text = c
                    .get("text")
                    .or_else(|| c.get("question"))
                    .and_then(Value::as_str)?;
                Some(format!("- {label}: {}", truncate_for_dossier(text, 240)))
            })
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            out.push_str("Archive signals:\n");
            out.push_str(&lines.join("\n"));
            out.push('\n');
        }
    }
    if let Some(results) = value.get("results").and_then(Value::as_array) {
        for result in results.iter().take(3) {
            let heading = recall_result_related_heading(result);
            let content = result
                .get("content")
                .or_else(|| result.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            out.push_str(&format!(
                "{heading}\n{}\n",
                truncate_for_dossier(content, result_chars)
            ));
        }
    }
    if out.trim().is_empty() {
        truncate_for_dossier(text, context_chars)
    } else {
        truncate_for_dossier(&out, context_chars)
    }
}

// ---------------------------------------------------------------------------
// Main hydration call (LLM)
// ---------------------------------------------------------------------------

fn run_prep_hydration_with_context(
    request: &PrepAiRequest,
    context: &PrepContext,
    steer_trailer: Option<&str>,
    pulled_texts: &[String],
) -> Result<HydrationResult, String> {
    // Any recall failure is explicit; quiet is reserved for successful,
    // grounded hydration with nothing useful to surface.
    if let Some(reason) = &context.catalyze_error {
        let state = "unavailable";
        let _ = append_prep_trace(
            request,
            json!({"kind":"hydration_skipped","reason":state,"lookup_error":reason}),
        );
        return Ok(HydrationResult {
            state: state.to_string(),
            posture: None,
            marginalia: vec![],
            reason: Some(reason.clone()),
            hint: Some("run margins setup".to_string()),
        });
    }

    // Build the prompt (cache-stable prefix then tail)
    let prompt = build_hydration_prompt(request, context, steer_trailer, pulled_texts);
    let _ = append_prep_trace(
        request,
        json!({"kind":"hydration_prompt","prompt_chars":prompt.chars().count()}),
    );

    let raw = prompt_prep_model_blocking(
        request,
        PREP_SYSTEM_PROMPT.to_string(),
        prompt,
        PREP_MAX_TOKENS,
    )?;

    let _ = append_prep_trace(
        request,
        json!({"kind":"hydration_response","raw":raw.clone()}),
    );

    // Parse
    let parsed: Value = match extract_json_object(&raw).and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(v) => v,
        None => {
            let _ = append_prep_trace(
                request,
                json!({"kind":"hydration_parse_failure","raw_chars":raw.len()}),
            );
            json!({"state":"thin","marginalia":[]})
        }
    };

    let state = parsed
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("thin")
        .to_string();
    let posture = parsed
        .get("posture")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
    let marginalia: Vec<Marginalia> = parsed
        .get("marginalia")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default();

    // Post-validation (non-negotiable)
    let validated = post_validate(
        marginalia,
        &request.sketch_text,
        request.vault_path.as_ref(),
        pulled_texts,
    );
    // State reflects whether we actually surfaced cards, not the model's self-label.
    // The model routinely under-labels a genuine 2-3 sourced-card result as "thin";
    // that previously hid real cards in the UI. Rule: any surviving marginalia →
    // "hydrated"; none → keep the honest empty state (thin/quiet) or downgrade an
    // over-claimed "hydrated" to "thin".
    let final_state = if validated.is_empty() {
        if state == "hydrated" {
            "thin".to_string()
        } else {
            state
        }
    } else {
        "hydrated".to_string()
    };

    Ok(HydrationResult {
        state: final_state,
        posture,
        marginalia: validated,
        reason: None,
        hint: None,
    })
}

/// Post-validation rules (enforced in Rust, not just in the prompt):
/// (a) Drop carried/counterevidence/context whose sources don't all resolve under the vault.
/// (b) If anchor is non-null but not a verbatim substring of sketch, set to null.
/// (c) Drop any item whose text is a near-match to a pulled text (already in user notes).
/// (d) Cap at 5.
pub(crate) fn post_validate(
    marginalia: Vec<Marginalia>,
    sketch_text: &str,
    vault_path: Option<&PathBuf>,
    pulled_texts: &[String],
) -> Vec<Marginalia> {
    let past_asserting_kinds = ["carried", "counterevidence", "context"];

    let mut result: Vec<Marginalia> = marginalia
        .into_iter()
        .map(|mut m| {
            // (b) anchor must be verbatim substring
            if let Some(ref anchor) = m.anchor.clone() {
                if !sketch_text.contains(anchor.as_str()) {
                    m.anchor = None;
                }
            }
            m
        })
        .filter(|m| {
            // (a) past-asserting kinds need all sources to resolve under vault
            let requires_sources = past_asserting_kinds.contains(&m.kind.as_str());
            if requires_sources {
                if m.sources.is_empty() {
                    return false;
                }
                let Some(vault) = vault_path else {
                    return false;
                };
                if !m.sources.iter().all(|src| {
                    let path = if std::path::Path::new(src).is_absolute() {
                        std::path::PathBuf::from(src)
                    } else {
                        vault.join(src)
                    };
                    path.exists()
                }) {
                    return false;
                }
            }

            // (c) drop near-matches to pulled texts
            if !pulled_texts.is_empty() {
                let norm_m = normalize_for_near_match(&m.text);
                for pulled in pulled_texts {
                    let norm_p = normalize_for_near_match(pulled);
                    if near_match(&norm_m, &norm_p) {
                        return false;
                    }
                }
            }

            true
        })
        .collect();

    // (d) cap at 5
    result.truncate(5);
    result
}

/// Normalize text for near-match comparison: lowercase, collapse whitespace, strip trailing punct.
fn normalize_for_near_match(text: &str) -> String {
    let lower = text.to_lowercase();
    // Collapse whitespace
    let collapsed = lower.split_whitespace().collect::<Vec<_>>().join(" ");
    // Strip trailing punctuation
    collapsed
        .trim_end_matches(|c: char| c.is_ascii_punctuation())
        .trim()
        .to_string()
}

/// True when two normalized strings are equal OR either is a substring of the other
/// with length ratio ≥ 0.6.
fn near_match(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if longer.contains(shorter) {
        let ratio = shorter.len() as f64 / longer.len() as f64;
        ratio >= 0.6
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Prompt construction
// ---------------------------------------------------------------------------

fn build_hydration_prompt(
    request: &PrepAiRequest,
    context: &PrepContext,
    steer_trailer: Option<&str>,
    pulled_texts: &[String],
) -> String {
    let instructions = if request.instructions.trim().is_empty() {
        "Act as a prep partner in the margin's register: spare, concrete, question-shaped with receipts. Prioritize carried items (deterministic, trust-anchored) and sketch-grounded question sharpening. Quiet is the common case; silence is what makes speech credible.".to_string()
    } else {
        request.instructions.clone()
    };

    let query_list = if context.queries.is_empty() {
        "(no queries)".to_string()
    } else {
        context
            .queries
            .iter()
            .map(|q| format!("- {q}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let attendees = if request.people.is_empty() {
        "(none)".to_string()
    } else {
        request.people.join(", ")
    };

    let event_line = request
        .event_title
        .as_deref()
        .map(|t| format!("Event: {t}\n"))
        .unwrap_or_default();

    // Cache-stable prefix: persona already in system prompt, user instructions, dossier, catalyze
    let mut prompt = format!(
        r#"You are the prep partner for Margins. The user is about to walk into a meeting.

Cache-stable context (reused across steering re-rolls):

User instructions:
{instructions}

Dossier — prior sessions and person notes (deterministic, no hallucination permitted):
{dossier}

Semantic context (vault search results):
{catalyze}

Search queries used:
{query_list}

---
Trigger-specific details (change per request):

{event_line}Attendees: {attendees}
"#,
        instructions = instructions,
        dossier = context.dossier,
        catalyze = context.catalyze_context,
        query_list = query_list,
        event_line = event_line,
        attendees = attendees,
    );

    // Mid-meeting context (R5): inserted after frozen dossier/catalyze, before sketch tail.
    if let Some(ref msf) = request.meeting_so_far {
        if !msf.trim().is_empty() {
            prompt.push_str(&format!(
                "\nMeeting so far (positioned context — the clock has been running; this is what has been discussed up to this pause):\n{msf}\n"
            ));
        }
    }

    prompt.push_str(&format!(
        r#"
The user's prep sketch (verbatim — never parse, summarize, or classify it):
---
{sketch}
---

Annotate this sketch as a sharp prep partner would: meet it at its own depth, attach each annotation to a specific sketch line (anchor) or to a gap (null anchor). Question-shaped with receipts. 2-3 items is typical; 5 maximum; quiet is valid.

For kind "carried", "counterevidence", or "context": sources MUST come from the dossier or search context above. Do not invent sources.
"#,
        sketch = request.sketch_text,
    ));

    // Pulled-text exclusion block (R4): before the schema.
    if !pulled_texts.is_empty() {
        prompt
            .push_str("\nAlready in the user's notes (do NOT re-offer these; they are handled):\n");
        for pt in pulled_texts {
            prompt.push_str(&format!("- {pt}\n"));
        }
        prompt.push('\n');
    }

    prompt.push_str(
        r#"Return ONLY strict JSON:
{
  "state": "hydrated | thin | quiet",
  "posture": "one line naming what this meeting is actually about, or empty string",
  "marginalia": [
    {
      "anchor": "verbatim substring of a sketch line, or null",
      "kind": "carried | blind_spot | sharpen | counterevidence | context",
      "text": "the annotation in margin register",
      "sources": ["vault-relative path", "..."]
    }
  ]
}
"#,
    );

    if let Some(trailer) = steer_trailer.map(str::trim).filter(|s| !s.is_empty()) {
        prompt.push('\n');
        prompt.push_str(trailer);
        prompt.push('\n');
    }

    prompt
}

/// Steering trailer (identical XML pattern to backchannel).
fn build_steer_trailer(
    original_marginalia: Option<&str>,
    history: &[SteerExchange],
    pending: &str,
) -> String {
    let mut out = String::new();
    out.push_str("<steering_session note=\"The user is refining the prep marginalia. Apply steers cumulatively; the pending steer matters most.\">\n");
    if let Some(orig) = original_marginalia.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "  <original_marginalia>{}</original_marginalia>\n",
            xml_escape(orig)
        ));
    }
    for (idx, exchange) in history.iter().enumerate() {
        out.push_str(&format!("  <revision index=\"{}\">\n", idx + 1));
        out.push_str(&format!(
            "    <instruction>{}</instruction>\n",
            xml_escape(exchange.instruction.trim())
        ));
        out.push_str(&format!(
            "    <result>{}</result>\n",
            xml_escape(exchange.result.trim())
        ));
        out.push_str("  </revision>\n");
    }
    out.push_str(&format!(
        "  <pending_instruction>{}</pending_instruction>\n",
        xml_escape(pending.trim())
    ));
    out.push_str("</steering_session>\n");
    out.push_str(
        "\nRegenerate marginalia that satisfies the full steer history. Honor the pending instruction most, keep earlier steers that do not conflict, and keep the same grounding, schema, and hard rules.",
    );
    out
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// ---------------------------------------------------------------------------
// Model call
// ---------------------------------------------------------------------------

fn prompt_prep_model_blocking(
    request: &PrepAiRequest,
    system_prompt: String,
    prompt: String,
    max_tokens: u32,
) -> Result<String, String> {
    let reactor = asupersync::runtime::reactor::create_reactor()
        .map_err(|e| format!("prep AI runtime: {e}"))?;
    let runtime = asupersync::runtime::RuntimeBuilder::current_thread()
        .with_reactor(reactor)
        .build()
        .map_err(|e| format!("prep AI runtime build: {e}"))?;
    let request_clone = request.clone();
    runtime.block_on(async move {
        prompt_prep_model(&request_clone, system_prompt, prompt, max_tokens).await
    })
}

async fn prompt_prep_model(
    request: &PrepAiRequest,
    system_prompt: String,
    prompt: String,
    max_tokens: u32,
) -> Result<String, String> {
    let started = Instant::now();
    let options = SessionOptions {
        provider: request.ai_provider.clone(),
        model: request.ai_model.clone(),
        api_key: request.ai_api_key.clone(),
        working_directory: Some(request.work_dir.clone()),
        no_session: true,
        session_dir: Some(request.margins_dir.join("ai-sessions")),
        enabled_tools: Some(Vec::new()),
        thinking: Some(ThinkingLevel::Off),
        max_tool_iterations: 0,
        append_system_prompt: Some(system_prompt),
        ..SessionOptions::default()
    };

    let mut session = create_agent_session_with_store(options, SessionStoreKind::Jsonl)
        .await
        .map_err(|e| {
            handle_prep_provider_error(request, "prep_setup");
            format!("prep AI setup failed: {e}")
        })?;
    session.set_max_tokens(Some(max_tokens));

    let first_delta_ms = Arc::new(Mutex::new(None::<u128>));
    let first_delta_for_events = Arc::clone(&first_delta_ms);
    let prompt_started = Instant::now();
    let assistant = session
        .prompt(prompt, move |event| {
            if note_text_delta(&event)
                .as_deref()
                .is_some_and(|d| !d.trim().is_empty())
            {
                if let Ok(mut first) = first_delta_for_events.lock() {
                    first.get_or_insert_with(|| prompt_started.elapsed().as_millis());
                }
            }
        })
        .await
        .map_err(|e| {
            handle_prep_provider_error(request, "prep_prompt");
            format!("prep AI failed: {e}")
        })?;

    let text = assistant_text(&assistant.content).trim().to_string();
    if text.is_empty() {
        return Err("prep AI returned no content".to_string());
    }

    let _ = append_prep_trace(
        request,
        json!({
            "kind": "model_timing",
            "elapsed_ms": started.elapsed().as_millis(),
            "first_delta_ms": first_delta_ms.lock().ok().and_then(|v| *v),
        }),
    );

    Ok(extract_json_object(&text).unwrap_or(text))
}

fn handle_prep_provider_error(
    request: &PrepAiRequest,
    stage: &'static str,
) -> crate::ai_config::ProviderErrorDisposition {
    crate::ai_config::handle_included_ai_provider_error(
        request.ai_credential_generation,
        None,
        stage,
    )
}

// ---------------------------------------------------------------------------
// Trace logging
// ---------------------------------------------------------------------------

fn append_prep_trace(request: &PrepAiRequest, event: Value) -> Result<(), String> {
    std::fs::create_dir_all(&request.margins_dir).map_err(|e| e.to_string())?;
    let path = request
        .margins_dir
        .join(format!("{}_prep_trace.jsonl", request.session_name));
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

// ---------------------------------------------------------------------------
// Utilities
// ---------------------------------------------------------------------------

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_open_fragments_finds_unchecked_items() {
        let note = "# Meeting notes\n\n- [x] Done thing\n- [ ] Follow up with Sam\n- [ ] Send the contract\n\n## Notes\n\nSome content.\n";
        let source = "[source: notes/meet.md · 2026-07-12 · 3 wks]";
        let fragments = extract_open_fragments(note, source);

        assert_eq!(fragments.len(), 2);
        assert!(fragments[0].contains("Follow up with Sam"));
        assert!(fragments[0].contains(source));
        assert!(fragments[1].contains("Send the contract"));
        // Completed items must not appear
        assert!(!fragments.iter().any(|f| f.contains("Done thing")));
    }

    #[test]
    fn extract_open_fragments_captures_open_question_sections() {
        let note = "# Meeting\n\n## Follow-up questions\n\nWhat is the pricing model?\nWho owns the decision?\n\n## Other\n\nSome other content.\n";
        let source = "[source: notes/meet.md]";
        let fragments = extract_open_fragments(note, source);

        // Should extract the two lines in the follow-up section
        assert!(fragments.iter().any(|f| f.contains("pricing model")));
        assert!(fragments.iter().any(|f| f.contains("owns the decision")));
        // Should not include the Other section
        assert!(!fragments.iter().any(|f| f.contains("Some other content")));
    }

    #[test]
    fn extract_open_fragments_ignores_checked_items_in_open_sections() {
        let note = "## Unresolved\n\n- [x] Already resolved\n- [ ] Still open\n";
        let source = "[source: x.md]";
        let fragments = extract_open_fragments(note, source);

        // The unchecked item in the unresolved section should appear
        assert!(fragments.iter().any(|f| f.contains("Still open")));
        // The checked item should not appear (filtered by `- [x]` guard)
        assert!(!fragments.iter().any(|f| f.contains("Already resolved")));
    }

    #[test]
    fn post_validate_drops_past_asserting_without_sources() {
        let vault = PathBuf::from("/tmp/vault");
        let marginalia = vec![
            Marginalia {
                anchor: None,
                kind: "carried".to_string(),
                text: "some claim".to_string(),
                sources: vec![], // no sources — must be dropped
            },
            Marginalia {
                anchor: None,
                kind: "sharpen".to_string(),
                text: "sharpen this".to_string(),
                sources: vec![],
            },
        ];
        let validated = post_validate(marginalia, "sketch text", Some(&vault), &[]);
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].kind, "sharpen");
    }

    #[test]
    fn post_validate_nulls_anchor_not_in_sketch() {
        let vault = PathBuf::from("/tmp/vault");
        let marginalia = vec![Marginalia {
            anchor: Some("word not in sketch".to_string()),
            kind: "sharpen".to_string(),
            text: "some sharpening".to_string(),
            sources: vec![],
        }];
        let validated = post_validate(
            marginalia,
            "completely different sketch text",
            Some(&vault),
            &[],
        );
        assert_eq!(validated.len(), 1);
        assert!(validated[0].anchor.is_none());
    }

    #[test]
    fn post_validate_keeps_valid_anchor() {
        let vault = PathBuf::from("/tmp/vault");
        let sketch = "discuss pricing with Sam";
        let marginalia = vec![Marginalia {
            anchor: Some("pricing".to_string()),
            kind: "sharpen".to_string(),
            text: "sharpen this".to_string(),
            sources: vec![],
        }];
        let validated = post_validate(marginalia, sketch, Some(&vault), &[]);
        assert_eq!(validated.len(), 1);
        assert_eq!(validated[0].anchor.as_deref(), Some("pricing"));
    }

    #[test]
    fn post_validate_caps_at_five() {
        let vault = PathBuf::from("/tmp/vault");
        let marginalia = (0..8)
            .map(|i| Marginalia {
                anchor: None,
                kind: "sharpen".to_string(),
                text: format!("item {i}"),
                sources: vec![],
            })
            .collect();
        let validated = post_validate(marginalia, "sketch", Some(&vault), &[]);
        assert_eq!(validated.len(), 5);
    }

    #[test]
    fn post_validate_drops_past_asserting_with_nonexistent_vault_paths() {
        let vault = PathBuf::from("/tmp/vault-does-not-exist-xyz");
        let marginalia = vec![Marginalia {
            anchor: None,
            kind: "counterevidence".to_string(),
            text: "some claim".to_string(),
            sources: vec!["people/Sam.md".to_string()],
        }];
        let validated = post_validate(marginalia, "sketch", Some(&vault), &[]);
        // Path doesn't exist → must be dropped
        assert!(validated.is_empty());
    }

    // R4 — pulled-text near-match exclusion tests
    #[test]
    fn post_validate_drops_exact_match_pulled_text() {
        let vault = PathBuf::from("/tmp/vault");
        let pulled = vec!["pricing — last time Sam pushed back on per-seat".to_string()];
        let marginalia = vec![Marginalia {
            anchor: None,
            kind: "sharpen".to_string(),
            text: "pricing — last time Sam pushed back on per-seat".to_string(),
            sources: vec![],
        }];
        let validated = post_validate(marginalia, "sketch", Some(&vault), &pulled);
        assert!(validated.is_empty(), "exact match should be dropped");
    }

    #[test]
    fn post_validate_drops_sixty_percent_substring_pulled_text() {
        let vault = PathBuf::from("/tmp/vault");
        // The shorter text "pricing thread with Sam" is a substring of the longer marginalia text,
        // and ratio ~ 23/40 = 0.575 — below threshold. Use one that's ≥ 0.6.
        // "pricing thread" (14) in "pricing thread with Sam" (23) → 14/23 = 0.61 ≥ 0.6.
        let pulled = vec!["pricing thread with Sam".to_string()];
        let marginalia = vec![Marginalia {
            anchor: None,
            kind: "sharpen".to_string(),
            text: "pricing thread".to_string(), // shorter, substring, ratio 14/23 ≥ 0.6
            sources: vec![],
        }];
        let validated = post_validate(marginalia, "sketch", Some(&vault), &pulled);
        assert!(validated.is_empty(), "60%+ substring should be dropped");
    }

    #[test]
    fn post_validate_keeps_unrelated_pulled_text() {
        let vault = PathBuf::from("/tmp/vault");
        let pulled = vec!["completely unrelated topic about widgets".to_string()];
        let marginalia = vec![Marginalia {
            anchor: None,
            kind: "sharpen".to_string(),
            text: "pricing thread with Sam".to_string(),
            sources: vec![],
        }];
        let validated = post_validate(marginalia, "sketch", Some(&vault), &pulled);
        assert_eq!(validated.len(), 1, "unrelated text should be kept");
    }

    #[test]
    fn session_age_label_parses_date_prefix() {
        // Use a very old date so the label is deterministic
        let label = session_age_label("2020-01-01-some-meeting");
        // Should be a multi-year label
        assert!(label.contains("yr") || label.contains("mo"), "got: {label}");
    }

    #[test]
    fn fallback_queries_uses_sketch_and_people() {
        let request = PrepAiRequest {
            request_id: "test".to_string(),
            work_dir: PathBuf::from("/tmp"),
            margins_dir: PathBuf::from("/tmp/.margins"),
            session_name: "test".to_string(),
            sketch_text: "discuss pricing".to_string(),
            people: vec!["Sam".to_string()],
            event_title: Some("Q3 Review".to_string()),
            vault_path: None,
            people_folder: "people".to_string(),
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: None,
            instructions: String::new(),
            block_ordinal: 0,
            pulled_texts: vec![],
            meeting_so_far: None,
        };
        let queries = build_fallback_queries(&request);
        assert!(!queries.is_empty());
        let joined = queries.join(" ");
        assert!(joined.contains("pricing") || joined.contains("Sam"));
    }

    #[test]
    fn compact_catalyze_context_uses_typed_evidence_labels() {
        let raw = serde_json::to_string(&json!({
            "schema_version": "margins.recall.v1",
            "status": "ok",
            "freshness": {"status":"stale","stale":true,"reason":"index lag","index":{"status":"stale","stale":true,"reason":"index lag"},"materialization":[]},
            "results": [
                {"document_ref":"people/sam.md","source_kind":"notes","evidence":{"kind":"native_markdown","path":"people/sam.md"},"content":"native hit"},
                {"document_ref":"mail:t1","source_kind":"google-mail","evidence":{"kind":"external_record","connector_id":"email","source_account":"a@b.com","source_id":"t1"},"content":"mail hit"}
            ]
        }))
        .unwrap();
        let compact = compact_catalyze_context(&raw, 600, 3000);
        assert!(compact.contains("[stale evidence: index lag]"));
        assert!(compact.contains("Related note: sam"));
        assert!(compact.contains("email thread t1"));
        assert!(!compact.contains("Related note: t1"));
    }

    #[test]
    fn fallback_queries_are_distinct_not_one_blob() {
        // A multi-line sketch must produce MULTIPLE separate queries (one per
        // line + attendees), not a single concatenated blob — otherwise a
        // reader-pass failure collapses retrieval to one weak query.
        let request = PrepAiRequest {
            request_id: "test".to_string(),
            work_dir: PathBuf::from("/tmp"),
            margins_dir: PathBuf::from("/tmp/.margins"),
            session_name: "test".to_string(),
            sketch_text: "pricing pushback from Sam\nonboarding gap flagged by two customers"
                .to_string(),
            people: vec!["Sam".to_string()],
            event_title: None,
            vault_path: None,
            people_folder: "people".to_string(),
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: None,
            instructions: String::new(),
            block_ordinal: 0,
            pulled_texts: vec![],
            meeting_so_far: None,
        };
        let queries = build_fallback_queries(&request);
        // 2 sketch lines + attendees = 3 distinct queries, none containing a newline.
        assert_eq!(queries.len(), 3);
        assert!(queries.iter().all(|q| !q.contains('\n')));
        assert!(queries.iter().any(|q| q.contains("pricing")));
        assert!(queries.iter().any(|q| q.contains("onboarding")));
    }
}
