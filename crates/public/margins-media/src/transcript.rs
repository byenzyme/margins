//! Deterministic transcript DTO adapters, phrase merging, and deduplication.

use serde::{Deserialize, Serialize};

/// Legacy-compatible word timing returned by Margins model adapters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WordTiming {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

impl From<&margins_core::TranscriptWord> for WordTiming {
    fn from(value: &margins_core::TranscriptWord) -> Self {
        Self {
            start_ms: value.start_ms,
            end_ms: value.end_ms,
            text: value.text.clone(),
        }
    }
}

impl From<WordTiming> for margins_core::TranscriptWord {
    fn from(value: WordTiming) -> Self {
        Self {
            start_ms: value.start_ms,
            end_ms: value.end_ms,
            text: value.text,
            speaker: None,
            confidence_per_mille: None,
        }
    }
}

/// Legacy-compatible channel-tagged transcript entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TranscriptWordEntry {
    pub channel: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub fn words_to_transcript_entries(
    words: &[WordTiming],
    channel: u32,
    offset_ms: u64,
) -> Vec<TranscriptWordEntry> {
    words
        .iter()
        .map(|word| TranscriptWordEntry {
            channel,
            start_ms: word.start_ms + offset_ms,
            end_ms: word.end_ms + offset_ms,
            text: format!(" {}", word.text.trim()),
        })
        .collect()
}

pub fn merge_word_entries_to_phrases(
    mut entries: Vec<TranscriptWordEntry>,
    max_gap_ms: u64,
) -> Vec<TranscriptWordEntry> {
    entries.sort_by_key(|entry| (entry.start_ms, entry.channel));
    let mut out: Vec<TranscriptWordEntry> = Vec::new();

    for entry in entries {
        if let Some(last) = out.last_mut() {
            if last.channel == entry.channel
                && entry.start_ms.saturating_sub(last.end_ms) <= max_gap_ms
            {
                last.end_ms = last.end_ms.max(entry.end_ms);
                last.text.push_str(&entry.text);
                continue;
            }
        }
        out.push(entry);
    }

    out
}

pub fn merge_and_dedupe_entries(
    mut entries: Vec<TranscriptWordEntry>,
    max_gap_ms: u64,
) -> Vec<TranscriptWordEntry> {
    entries.sort_by_key(|entry| (entry.start_ms, entry.channel));
    let mut deduped: Vec<TranscriptWordEntry> = Vec::new();
    let mut run: Vec<TranscriptWordEntry> = Vec::new();
    let mut run_token = String::new();

    for entry in entries {
        let token = normalized_dedupe_token(&entry.text);
        let same_run = run
            .last()
            .map(|last| last.channel == entry.channel && run_token == token)
            .unwrap_or(false);

        if same_run {
            run.push(entry);
        } else {
            flush_dedupe_run(&mut deduped, &mut run, &run_token);
            run_token = token;
            run.push(entry);
        }
    }
    flush_dedupe_run(&mut deduped, &mut run, &run_token);

    merge_word_entries_to_phrases(deduped, max_gap_ms)
}

fn normalized_dedupe_token(text: &str) -> String {
    text.trim()
        .trim_end_matches(['.', ',', '!', '?'])
        .to_ascii_lowercase()
}

fn is_dedupe_filler_token(token: &str) -> bool {
    matches!(token, "like" | "yeah" | "the" | "um" | "uh" | "so")
}

fn flush_dedupe_run(
    out: &mut Vec<TranscriptWordEntry>,
    run: &mut Vec<TranscriptWordEntry>,
    token: &str,
) {
    if run.is_empty() {
        return;
    }

    if !token.is_empty() && (run.len() >= 3 || is_dedupe_filler_token(token)) {
        let mut first = run.remove(0);
        for entry in run.drain(..) {
            first.end_ms = first.end_ms.max(entry.end_ms);
        }
        out.push(first);
    } else {
        out.append(run);
    }
}

pub fn transcript_json(entries: &[TranscriptWordEntry]) -> serde_json::Value {
    serde_json::json!({
        "transcripts": [{
            "words": entries,
        }]
    })
}
