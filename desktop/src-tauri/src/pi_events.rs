use pi::sdk::{AgentEvent, ContentBlock, ToolOutput};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

type Emit = dyn Fn(&str, &str, Option<f32>) + Send + Sync;

pub(crate) fn debug_raw_trace_file(
    margins_dir: &Path,
    session_name: &str,
) -> Option<Arc<Mutex<File>>> {
    let enabled = cfg!(debug_assertions)
        || std::env::var("MARGINS_DEBUG_TRACES")
            .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
    if !enabled {
        return None;
    }
    std::fs::create_dir_all(margins_dir).ok()?;
    let path = margins_dir.join(format!("{session_name}_pi_raw_trace.jsonl"));
    File::create(path)
        .ok()
        .map(|file| Arc::new(Mutex::new(file)))
}

fn write_debug_raw_trace(raw_trace: &Option<Arc<Mutex<File>>>, event: &AgentEvent) {
    let Some(file) = raw_trace else {
        return;
    };
    let Ok(mut guard) = file.lock() else {
        return;
    };
    let Ok(line) = serde_json::to_string(event) else {
        return;
    };
    let _ = writeln!(guard, "{line}");
}

pub(crate) fn emit_pi_event(
    emit: &Emit,
    tool_labels: &Arc<Mutex<HashMap<String, String>>>,
    raw_trace: &Option<Arc<Mutex<File>>>,
    event: AgentEvent,
) {
    write_debug_raw_trace(raw_trace, &event);
    match event {
        AgentEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => {
            if let Ok(value) = serde_json::to_value(&assistant_message_event) {
                match value.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                            emit("note_stream", delta, None);
                        }
                    }
                    Some("text_end") => {
                        if !value
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .ends_with('\n')
                        {
                            emit("note_stream", "\n", None);
                        }
                    }
                    _ => {}
                }
            }
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            let label = tool_trace_label(&tool_name, &args);
            let summary = summarize_tool_args(&tool_name, &args);
            if let Ok(mut labels) = tool_labels.lock() {
                labels.insert(tool_call_id, label.clone());
            }
            emit(
                "synthesize",
                &format!("Tool start: {label} — {summary}"),
                None,
            );
        }
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => {
            let label = tool_labels
                .lock()
                .ok()
                .and_then(|mut labels| labels.remove(&tool_call_id))
                .unwrap_or_else(|| tool_trace_label(&tool_name, &Value::Null));
            let summary = summarize_tool_output(&tool_name, &result);
            let verb = if is_error { "failed" } else { "done" };
            emit(
                "synthesize",
                &format!("Tool {verb}: {label} — {summary}"),
                None,
            );
        }
        AgentEvent::TurnStart { turn_index, .. } => {
            emit(
                "synthesize",
                &format!(
                    "Thinking pass {}: reading context and planning note work",
                    turn_index + 1
                ),
                None,
            );
        }
        AgentEvent::AgentEnd {
            error: Some(error), ..
        } => {
            emit(
                "synthesize",
                &format!("Note writer finished with warning: {error}"),
                None,
            );
        }
        AgentEvent::AgentEnd { .. } => {
            emit(
                "synthesize",
                "Note writer finished drafting and saving the note",
                Some(0.88),
            );
        }
        _ => {}
    }
}

pub(crate) fn note_text_delta(event: &AgentEvent) -> Option<String> {
    let AgentEvent::MessageUpdate {
        assistant_message_event,
        ..
    } = event
    else {
        return None;
    };
    let value = serde_json::to_value(assistant_message_event).ok()?;
    match value.get("type").and_then(Value::as_str) {
        Some("text_delta") => value
            .get("delta")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        Some("text_end") => {
            if value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .ends_with('\n')
            {
                None
            } else {
                Some("\n".to_string())
            }
        }
        _ => None,
    }
}

fn tool_trace_label(tool_name: &str, args: &Value) -> String {
    match tool_name {
        "enzyme_petri" => "scanning your notes".to_string(),
        "enzyme_catalyze" => args
            .get("query")
            .and_then(Value::as_str)
            .map(|q| format!("finding related ideas: {}", truncate(q, 48)))
            .unwrap_or_else(|| "finding related ideas".to_string()),
        "read" => args
            .get("path")
            .and_then(Value::as_str)
            .map(|p| format!("read: {}", truncate_path(p)))
            .unwrap_or_else(|| "read file".to_string()),
        "grep" => args
            .get("pattern")
            .and_then(Value::as_str)
            .map(|p| format!("grep: {}", truncate(p, 48)))
            .unwrap_or_else(|| "grep files".to_string()),
        "find" => args
            .get("path")
            .and_then(Value::as_str)
            .map(|p| format!("find: {}", truncate_path(p)))
            .unwrap_or_else(|| "find files".to_string()),
        "ls" => args
            .get("path")
            .and_then(Value::as_str)
            .map(|p| format!("ls: {}", truncate_path(p)))
            .unwrap_or_else(|| "list files".to_string()),
        other => other.to_string(),
    }
}

fn summarize_tool_args(tool_name: &str, args: &Value) -> String {
    match tool_name {
        "enzyme_catalyze" => args
            .get("query")
            .and_then(Value::as_str)
            .map(|q| format!("search query: {}", truncate(q, 120)))
            .unwrap_or_else(|| "searching for related evidence".to_string()),
        "enzyme_petri" => "checking your notes for related ideas".to_string(),
        "read" => args
            .get("path")
            .and_then(Value::as_str)
            .map(|p| format!("reading {}", truncate_path(p)))
            .unwrap_or_else(|| "reading supporting evidence".to_string()),
        "grep" => args
            .get("pattern")
            .and_then(Value::as_str)
            .map(|p| format!("searching files for {}", truncate(p, 80)))
            .unwrap_or_else(|| "searching supporting files".to_string()),
        _ => truncate(&args.to_string(), 160).replace(['\n', '\r'], " "),
    }
}

fn summarize_tool_output(tool_name: &str, output: &ToolOutput) -> String {
    if output.is_error {
        return tool_output_text(output)
            .map(|s| format!("error: {}", truncate(&s, 160)))
            .unwrap_or_else(|| "tool returned an error".to_string());
    }

    if let Some(details) = &output.details {
        if let Some(path) = details.get("note_path").and_then(Value::as_str) {
            return format!("saved to {}", truncate_path(path));
        }
    }

    // Catalyst recall returns evidence connected through shared ideas. Surface
    // readable evidence labels so the step communicates what was found.
    if tool_name == "enzyme_catalyze" {
        if let Some(summary) = enzyme_connection_summary(output) {
            return summary;
        }
    }
    if tool_name == "enzyme_petri" {
        return "mapped your notes' ideas, ready to search".to_string();
    }

    tool_output_text(output)
        .map(|s| {
            let lines = s.lines().filter(|line| !line.trim().is_empty()).count();
            if lines > 1 {
                format!(
                    "returned {lines} lines: {}",
                    truncate(&s.replace('\n', " "), 140)
                )
            } else {
                truncate(&s, 160)
            }
        })
        .unwrap_or_else(|| "completed".to_string())
}

/// Concise stale signal for recall JSON compaction when freshness is degraded.
pub(crate) fn recall_freshness_stale_prefix(value: &Value) -> String {
    let Some(freshness) = value.get("freshness") else {
        return String::new();
    };
    if freshness.get("stale").and_then(Value::as_bool) != Some(true) {
        return String::new();
    }
    match freshness
        .get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
    {
        Some(reason) => format!("[stale evidence: {reason}] "),
        None => "[stale evidence] ".to_string(),
    }
}

/// Readable label for a Margins recall result. Never treats `document_ref` as a path.
pub(crate) fn recall_result_label(result: &Value) -> String {
    let evidence = result.get("evidence");
    match evidence.and_then(|e| e.get("kind")).and_then(Value::as_str) {
        Some("native_markdown") => {
            let path = evidence
                .and_then(|e| e.get("path"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if path.is_empty() {
                "related evidence".to_string()
            } else {
                note_title_from_path(path)
            }
        }
        Some("external_record") => {
            let source_kind = result
                .get("source_kind")
                .and_then(Value::as_str)
                .unwrap_or("");
            let source_id = evidence
                .and_then(|e| e.get("source_id"))
                .and_then(Value::as_str)
                .unwrap_or("record");
            external_recall_label(source_kind, source_id)
        }
        _ => "related evidence".to_string(),
    }
}

/// Prompt-oriented heading: native hits stay note-labeled; external hits never are.
pub(crate) fn recall_result_related_heading(result: &Value) -> String {
    match result
        .get("evidence")
        .and_then(|e| e.get("kind"))
        .and_then(Value::as_str)
    {
        Some("native_markdown") => format!("Related note: {}", recall_result_label(result)),
        Some("external_record") => recall_result_label(result),
        _ => "Related evidence".to_string(),
    }
}

fn external_recall_label(source_kind: &str, source_id: &str) -> String {
    match source_kind {
        "google-mail" => format!("email thread {source_id}"),
        "google-calendar" => format!("calendar event {source_id}"),
        other if !other.is_empty() => format!("{other} {source_id}"),
        _ => format!("external record {source_id}"),
    }
}

fn tool_output_text(output: &ToolOutput) -> Option<String> {
    let text = output
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Parse a typed recall result payload and describe the connection using native
/// note titles or external-record labels. Returns None when the payload isn't
/// the expected JSON shape so the caller can fall back to the generic summary.
fn enzyme_connection_summary(output: &ToolOutput) -> Option<String> {
    let text = tool_output_text(output)?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let results = value.get("results")?.as_array()?;
    let freshness_prefix = recall_freshness_stale_prefix(&value);
    if results.is_empty() {
        return Some(format!(
            "{freshness_prefix}no closely related evidence found"
        ));
    }
    let mut titles: Vec<String> = Vec::new();
    for result in results {
        let title = recall_result_label(result);
        if title != "related evidence" && !title.is_empty() && !titles.iter().any(|t| t == &title) {
            titles.push(title);
        }
    }
    if titles.is_empty() {
        return None;
    }
    let shown = titles.iter().take(4).cloned().collect::<Vec<_>>();
    let extra = titles.len() - shown.len();
    let mut summary = format!("{freshness_prefix}connected to {}", shown.join(", "));
    if extra > 0 {
        summary.push_str(&format!(" +{extra} more"));
    }
    Some(summary)
}

/// Turn a note path into a readable title: the file stem with any leading
/// `YYYY-MM-DD[-HH-MM-SS]` capture stamp stripped off.
fn note_title_from_path(path: &str) -> String {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let bytes = stem.as_bytes();
    let looks_dated =
        bytes.len() >= 5 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'-';
    let cleaned = if looks_dated {
        let rest = stem.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-' || c == ' ');
        if rest.is_empty() {
            stem
        } else {
            rest
        }
    } else {
        stem
    };
    truncate(cleaned, 42)
}

fn truncate(input: &str, max_chars: usize) -> String {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        format!(
            "{}…",
            normalized
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn truncate_path(path: &str) -> String {
    let p = Path::new(path);
    let file = p.file_name().and_then(|s| s.to_str()).unwrap_or(path);
    let parent = p
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str());
    match parent {
        Some(parent) => format!("{parent}/{file}"),
        None => file.to_string(),
    }
}

pub(crate) fn assistant_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi::sdk::TextContent;

    fn catalyze_output(json: &str) -> ToolOutput {
        ToolOutput {
            content: vec![ContentBlock::Text(TextContent::new(json))],
            details: None,
            is_error: false,
        }
    }

    #[test]
    fn recall_result_label_handles_native_and_external_evidence() {
        let native = serde_json::json!({
            "document_ref":"people/jane.md",
            "source_kind":"notes",
            "evidence":{"kind":"native_markdown","path":"/vault/2026-06-18 operating cadence.md"}
        });
        let external = serde_json::json!({
            "document_ref":"mail:thread-1",
            "source_kind":"google-mail",
            "evidence":{"kind":"external_record","connector_id":"email","source_account":"owner@example.com","source_id":"thread-1"}
        });
        assert_eq!(recall_result_label(&native), "operating cadence");
        assert_eq!(recall_result_label(&external), "email thread thread-1");
        assert_eq!(
            recall_result_related_heading(&native),
            "Related note: operating cadence"
        );
        assert_eq!(
            recall_result_related_heading(&external),
            "email thread thread-1"
        );
    }

    #[test]
    fn recall_freshness_stale_prefix_includes_reason_when_present() {
        let value = serde_json::json!({
            "freshness":{"status":"stale","stale":true,"reason":"index behind ledger"}
        });
        assert_eq!(
            recall_freshness_stale_prefix(&value),
            "[stale evidence: index behind ledger] "
        );
    }

    #[test]
    fn note_title_strips_capture_datestamp() {
        assert_eq!(
            note_title_from_path(
                "/Users/x/obsidian/inbox/2026-06-18-11-56-17 agent plaza village spec.md"
            ),
            "agent plaza village spec"
        );
        assert_eq!(
            note_title_from_path("/Users/x/obsidian/people/Timour.md"),
            "Timour"
        );
    }

    #[test]
    fn enzyme_summary_lists_connected_note_titles() {
        let output = catalyze_output(
            r#"{"schema_version":"margins.recall.v1","status":"ok","results":[
                {"document_ref":"inbox/op.md","source_kind":"notes","evidence":{"kind":"native_markdown","path":"/v/inbox/2026-06-18 operating cadence.md"},"similarity":0.9},
                {"document_ref":"inbox/pilot.md","source_kind":"notes","evidence":{"kind":"native_markdown","path":"/v/inbox/2026-06-17 pilot design.md"},"similarity":0.8}
            ]}"#,
        );
        assert_eq!(
            summarize_tool_output("enzyme_catalyze", &output),
            "connected to operating cadence, pilot design"
        );
    }

    #[test]
    fn enzyme_summary_labels_external_records_without_note_paths() {
        let output = catalyze_output(
            r#"{"schema_version":"margins.recall.v1","status":"ok","results":[
                {"document_ref":"mail:thread-1","source_kind":"google-mail","evidence":{"kind":"external_record","connector_id":"email","source_account":"owner@example.com","source_id":"thread-1"},"similarity":0.7}
            ]}"#,
        );
        assert_eq!(
            summarize_tool_output("enzyme_catalyze", &output),
            "connected to email thread thread-1"
        );
    }

    #[test]
    fn enzyme_summary_preserves_stale_freshness_for_visible_tool_status() {
        let output = catalyze_output(
            r#"{"schema_version":"margins.recall.v1","status":"ok","freshness":{"status":"error","stale":true,"reason":"refresh_failed"},"results":[
                {"document_ref":"mail:thread-1","source_kind":"google-mail","evidence":{"kind":"external_record","connector_id":"email","source_account":"owner@example.com","source_id":"thread-1"},"similarity":0.7}
            ]}"#,
        );
        assert_eq!(
            summarize_tool_output("enzyme_catalyze", &output),
            "[stale evidence: refresh_failed] connected to email thread thread-1"
        );
    }

    #[test]
    fn enzyme_summary_caps_and_counts_extra() {
        let output = catalyze_output(
            r#"{"results":[
                {"evidence":{"kind":"native_markdown","path":"a one.md"}},
                {"evidence":{"kind":"native_markdown","path":"b two.md"}},
                {"evidence":{"kind":"native_markdown","path":"c three.md"}},
                {"evidence":{"kind":"native_markdown","path":"d four.md"}},
                {"evidence":{"kind":"native_markdown","path":"e five.md"}},
                {"evidence":{"kind":"native_markdown","path":"f six.md"}}
            ]}"#,
        );
        assert_eq!(
            summarize_tool_output("enzyme_catalyze", &output),
            "connected to a one, b two, c three, d four +2 more"
        );
    }

    #[test]
    fn enzyme_summary_handles_empty_results() {
        let output = catalyze_output(r#"{"results":[]}"#);
        assert_eq!(
            summarize_tool_output("enzyme_catalyze", &output),
            "no closely related evidence found"
        );
    }
}
