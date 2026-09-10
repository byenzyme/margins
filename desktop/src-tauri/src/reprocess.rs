//! One-shot speaker-attribution rewrite using the live-cues "fast model".
//!
//! `run_reprocess` takes the existing vault note for a session, gathers RAG
//! context for each named person (enzyme petri + person note excerpt), then
//! asks the fast model to rewrite ONLY the speaker/person attributions — preserving
//! every other element of structure, wording, and order.

use margins::session;
use pi::sdk::{
    create_agent_session_with_store, ContentBlock, SessionOptions, SessionStoreKind, ThinkingLevel,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::pi_events::note_text_delta;

pub struct ReprocessRequest {
    pub session_name: String,
    pub people: Vec<String>,
    pub work_dir: PathBuf,
    pub margins_dir: PathBuf,
    pub vault_path: Option<PathBuf>,
    pub people_folder: String,
    pub ai_provider: Option<String>,
    /// The "fast model" resolved from the live-cues/backchannel config. Reprocess
    /// shares whatever model live cues uses (Haiku today); it is not hardcoded.
    pub ai_model: Option<String>,
    pub ai_api_key: Option<String>,
    pub ai_credential_generation: Option<u64>,
}

pub fn run_reprocess(
    request: ReprocessRequest,
    emit: impl Fn(&str, &str, Option<f32>) + Send + Sync + 'static,
) -> Result<(), String> {
    let reactor = asupersync::runtime::reactor::create_reactor()
        .map_err(|e| format!("failed to create AI runtime: {e}"))?;
    let runtime = asupersync::runtime::RuntimeBuilder::current_thread()
        .with_reactor(reactor)
        .build()
        .map_err(|e| format!("failed to build AI runtime: {e}"))?;
    runtime.block_on(run_reprocess_async(request, Arc::new(emit)))
}

async fn run_reprocess_async(
    request: ReprocessRequest,
    emit: Arc<dyn Fn(&str, &str, Option<f32>) + Send + Sync>,
) -> Result<(), String> {
    emit("reprocess", "Reading existing note...", Some(0.1));

    // 1. Read the existing note from the vault note path.
    let meta = session::get_session_meta(&request.margins_dir, &request.session_name)
        .map_err(|e| e.to_string())?;
    let note_path = meta
        .vault_note_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .ok_or_else(|| "Save a note before reprocessing with people.".to_string())?;
    let current_note =
        std::fs::read_to_string(&note_path).map_err(|e| format!("Failed to read note: {e}"))?;

    emit(
        "reprocess",
        &format!(
            "Gathering context for {} person(s)...",
            request.people.len()
        ),
        Some(0.2),
    );

    // 2. Build RAG context per person.
    let rag_context = build_rag_context(
        &request.people,
        request.vault_path.as_deref(),
        &request.people_folder,
    );

    emit("reprocess", "Sending rewrite request...", Some(0.35));

    // 3. Build and run the fast-model session.
    let _api_key_env = ScopedEnvVar::set("MARGINS_API_KEY", request.ai_api_key.as_deref());

    let options = SessionOptions {
        provider: request.ai_provider.clone(),
        model: request.ai_model.clone(),
        api_key: request.ai_api_key.clone(),
        working_directory: Some(request.work_dir.clone()),
        no_session: true,
        session_path: None,
        session_dir: None,
        // No tools — one-shot rewrite only.
        enabled_tools: Some(vec![]),
        thinking: Some(ThinkingLevel::Low),
        max_tool_iterations: 0,
        tool_factory: None,
        append_system_prompt: Some(
            "You are running inside Margins Desktop. Output ONLY the rewritten Markdown note — no preamble, no explanation, no code fences.".to_string(),
        ),
        ..SessionOptions::default()
    };

    let mut session = create_agent_session_with_store(options, SessionStoreKind::Jsonl)
        .await
        .map_err(|e| {
            handle_reprocess_provider_error(&request, "reprocess_setup");
            format!("AI model setup failed: {e}")
        })?;

    let prompt = build_reprocess_prompt(&request.people, &rag_context, &current_note);

    let accumulated = Arc::new(Mutex::new(String::new()));
    let accumulated_for_events = accumulated.clone();
    let emit_for_events = emit.clone();

    let assistant = session
        .prompt(prompt, move |event| {
            if let Some(delta) = note_text_delta(&event) {
                if let Ok(mut buf) = accumulated_for_events.lock() {
                    buf.push_str(&delta);
                }
                emit_for_events("note_stream", &delta, None);
            }
        })
        .await
        .map_err(|e| {
            handle_reprocess_provider_error(&request, "reprocess_prompt");
            format!("Reprocess failed: {e}")
        })?;

    // Extract final text (streaming may have captured it; fall back to full response).
    let rewritten = {
        let buf = accumulated
            .lock()
            .ok()
            .map(|g| g.trim().to_string())
            .unwrap_or_default();
        if buf.is_empty() {
            // Fallback: pull text from assistant content blocks.
            assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        } else {
            buf
        }
    };

    if rewritten.is_empty() {
        return Err("Reprocess returned an empty response — note not overwritten.".to_string());
    }

    // 4. Overwrite the note file.
    std::fs::write(&note_path, &rewritten)
        .map_err(|e| format!("Failed to write reprocessed note: {e}"))?;

    emit("complete", "Reprocess complete!", Some(1.0));
    Ok(())
}

fn handle_reprocess_provider_error(
    request: &ReprocessRequest,
    stage: &'static str,
) -> crate::ai_config::ProviderErrorDisposition {
    crate::ai_config::handle_included_ai_provider_error(
        request.ai_credential_generation,
        None,
        stage,
    )
}

// ---------------------------------------------------------------------------
// RAG context helpers
// ---------------------------------------------------------------------------

fn build_rag_context(people: &[String], vault_path: Option<&Path>, people_folder: &str) -> String {
    let mut sections: Vec<String> = Vec::new();

    for person in people {
        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("## Person: {person}"));

        // (a) Enzyme petri for this person.
        if let Some(vault) = vault_path {
            let petri_out = run_enzyme(
                vec![
                    "petri".to_string(),
                    "--vault".to_string(),
                    vault.to_string_lossy().to_string(),
                    "--query".to_string(),
                    person.clone(),
                    "--top".to_string(),
                    "3".to_string(),
                    "--catalyst-budget".to_string(),
                    "8".to_string(),
                ],
                Some(vault),
            );
            match petri_out {
                Ok(run) if !run.stdout.trim().is_empty() => {
                    let catalysts = extract_petri_catalysts(&run.stdout, 6);
                    if !catalysts.is_empty() {
                        parts.push(format!("### Vault catalysts for {person}\n{catalysts}"));
                    }
                }
                _ => {
                    // Petri unavailable or failed — skip gracefully.
                }
            }
        }

        // (b) Person note file excerpt.
        if let Some(vault) = vault_path {
            let person_note_path = vault.join(people_folder).join(format!("{person}.md"));
            if person_note_path.exists() {
                if let Ok(contents) = std::fs::read_to_string(&person_note_path) {
                    let excerpt = truncate_chars(contents.trim(), 1500);
                    if !excerpt.is_empty() {
                        parts.push(format!("### Note file for {person}\n{excerpt}"));
                    }
                }
            }
        }

        sections.push(parts.join("\n\n"));
    }

    sections.join("\n\n---\n\n")
}

/// Parse petri JSON and extract catalyst text (+ optional context) strings,
/// up to `max` entries.
fn extract_petri_catalysts(stdout: &str, max: usize) -> String {
    let Ok(value) = serde_json::from_str::<Value>(stdout) else {
        // Not JSON — return raw (truncated).
        return truncate_chars(stdout, 1200);
    };
    let mut lines: Vec<String> = Vec::new();
    if let Some(entities) = value.get("entities").and_then(Value::as_array) {
        'outer: for entity in entities {
            let entity_name = entity
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("(unnamed)");
            if let Some(catalysts) = entity.get("catalysts").and_then(Value::as_array) {
                for catalyst in catalysts {
                    if lines.len() >= max {
                        break 'outer;
                    }
                    let text = catalyst.get("text").and_then(Value::as_str).unwrap_or("");
                    let context = catalyst
                        .get("context")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if text.trim().is_empty() {
                        continue;
                    }
                    if context.trim().is_empty() {
                        lines.push(format!("- [{entity_name}] {text}"));
                    } else {
                        lines.push(format!("- [{entity_name}] {text} (context: {context})"));
                    }
                }
            }
        }
    }
    lines.join("\n")
}

fn build_reprocess_prompt(people: &[String], rag_context: &str, current_note: &str) -> String {
    let people_list = people
        .iter()
        .map(|p| format!("- {p}"))
        .collect::<Vec<_>>()
        .join("\n");

    let rag_section = if rag_context.trim().is_empty() {
        "(No vault context available.)".to_string()
    } else {
        rag_context.to_string()
    };

    format!(
        r#"You are a careful editor fixing speaker/person attributions in a meeting note. Your edits should read as if the note had been written by someone who knew everyone's names all along — natural, not mechanical.

## Instructions

- PRESERVE every heading, section, paragraph, bullet, wikilink, tag, YAML frontmatter, and action item EXACTLY as written.
- Use the vault context to work out who each person is and what role they play, so you assign attributions correctly.
- Replace generic or wrong speaker labels ("Speaker 2", "the other person", "the speaker") with the correct named person from the list below.
- Write the way people naturally do: name a person at their first mention in a section, then use pronouns. Do NOT replace a pronoun with a name when the referent is already clear from context — a note that repeats a name in every sentence reads worse, not better.
- If an attribution is already clear and natural, leave it untouched.
- Do NOT reword, summarize, expand, add, or delete any content beyond attribution fixes.
- Output ONLY the complete rewritten Markdown note. No preamble, no explanation, no code fences.

## Named people for this session

{people_list}

## Vault context per person

{rag_section}

## Current note to rewrite

{current_note}
"#
    )
}

// ---------------------------------------------------------------------------
// Enzyme subprocess helper (mirrors pi_distill::run_enzyme)
// ---------------------------------------------------------------------------

struct EnzymeOutput {
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

// In-process petri/catalyze via the vendored recall engine (no subprocess).
fn run_enzyme(args: Vec<String>, _work_dir: Option<&Path>) -> Result<EnzymeOutput, EnzymeOutput> {
    match crate::recall_search_compat(&args) {
        Ok(stdout) => Ok(EnzymeOutput {
            stdout,
            stderr: String::new(),
        }),
        Err(stderr) => Err(EnzymeOutput {
            stdout: String::new(),
            stderr,
        }),
    }
}

fn truncate_chars(input: &str, max: usize) -> String {
    if input.chars().count() <= max {
        input.to_string()
    } else {
        format!(
            "{}…",
            input
                .chars()
                .take(max.saturating_sub(1))
                .collect::<String>()
        )
    }
}

// ---------------------------------------------------------------------------
// Scoped env-var (mirrors pi_distill::ScopedEnvVar usage)
// ---------------------------------------------------------------------------

struct ScopedEnvVar {
    name: &'static str,
    prev: Option<String>,
    active: bool,
}

impl ScopedEnvVar {
    fn set(name: &'static str, value: Option<&str>) -> Self {
        let prev = std::env::var(name).ok();
        match value {
            Some(v) if !v.is_empty() => {
                std::env::set_var(name, v);
                Self {
                    name,
                    prev,
                    active: true,
                }
            }
            _ => Self {
                name,
                prev,
                active: false,
            },
        }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        match &self.prev {
            Some(v) => std::env::set_var(self.name, v),
            None => std::env::remove_var(self.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reprocess_provider_failure_invalidates_propagated_generation() {
        let generation = crate::ai_config::install_test_included_lease_generation();
        let request = ReprocessRequest {
            session_name: "generation-test".to_string(),
            people: Vec::new(),
            work_dir: PathBuf::from("/tmp/work"),
            margins_dir: PathBuf::from("/tmp/work/.margins"),
            vault_path: None,
            people_folder: "people".to_string(),
            ai_provider: None,
            ai_model: None,
            ai_api_key: None,
            ai_credential_generation: Some(generation),
        };
        assert_eq!(
            handle_reprocess_provider_error(&request, "reprocess_prompt"),
            crate::ai_config::ProviderErrorDisposition::Invalidated
        );
        assert!(!crate::ai_config::test_included_lease_generation_is_usable(
            generation
        ));
    }
}
