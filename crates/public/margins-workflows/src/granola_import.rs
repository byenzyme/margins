use crate::publish;
use anyhow::{bail, Context, Result as AnyResult};
use chrono::{DateTime, Local};
use margins_store::legacy as session;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GranolaImportSurvey {
    pub file_count: usize,
    pub meeting_count: usize,
    pub people: Vec<String>,
    pub organizations: Vec<String>,
    pub suggested_notes_folder: String,
    pub suggested_people_folder: String,
    pub suggested_organizations_folder: String,
    pub suggested_transcripts_folder: String,
    pub folder_candidates: Vec<String>,
    pub sample_titles: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GranolaImportOptions {
    pub notes_folder: String,
    pub people_folder: String,
    pub organizations_folder: String,
    pub transcripts_folder: String,
    #[serde(default)]
    pub write_transcript_files: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GranolaImportResult {
    pub imported_count: usize,
    #[serde(default)]
    pub updated_count: usize,
    pub note_paths: Vec<String>,
    pub people_created: usize,
    pub organizations_created: usize,
    pub transcript_paths: Vec<String>,
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub session_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GranolaVaultTarget {
    pub vault_root: PathBuf,
    pub notes_folder: String,
    pub people_folder: String,
    pub organizations_folder: String,
}

#[derive(Clone, Debug)]
pub struct Person {
    pub name: String,
    pub email: Option<String>,
    pub organizations: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Meeting {
    pub id: Option<String>,
    pub title: String,
    pub created_at: Option<String>,
    pub notes: Option<String>,
    pub transcript: Option<String>,
    pub people: Vec<Person>,
    pub organizations: Vec<String>,
    /// Set by MCP sync when the transcript fetch was blocked by a plan gate
    /// (e.g. Granola's API requires a paid tier to expose transcripts).
    /// File-import paths always leave this `false`.
    pub plan_gated_transcript: bool,
}

pub fn survey(
    paths: &[String],
    vault_root: &Path,
    inbox_folder: &str,
    people_folder: &str,
) -> Result<GranolaImportSurvey, String> {
    let (meetings, warnings) = read_meetings(paths)?;
    let mut people = BTreeSet::new();
    let mut orgs = BTreeSet::new();
    for meeting in &meetings {
        for person in &meeting.people {
            people.insert(person.name.clone());
        }
        for org in &meeting.organizations {
            orgs.insert(org.clone());
        }
    }
    let candidates = folder_candidates(vault_root);
    Ok(GranolaImportSurvey {
        file_count: paths.len(),
        meeting_count: meetings.len(),
        people: people.into_iter().collect(),
        organizations: orgs.into_iter().collect(),
        suggested_notes_folder: folder_or_default(inbox_folder, "meetings"),
        suggested_people_folder: folder_or_default(people_folder, "people"),
        suggested_organizations_folder: choose_folder(
            &candidates,
            &["organizations", "orgs", "companies"],
        )
        .unwrap_or_else(|| "organizations".to_string()),
        suggested_transcripts_folder: choose_folder(
            &candidates,
            &["transcripts", "meetings/transcripts", "raw transcripts"],
        )
        .unwrap_or_else(|| "transcripts".to_string()),
        folder_candidates: candidates,
        sample_titles: meetings.iter().take(4).map(|m| m.title.clone()).collect(),
        warnings,
    })
}

pub fn import(
    paths: &[String],
    vault_root: &Path,
    margins_dir: &Path,
    options: &GranolaImportOptions,
) -> Result<GranolaImportResult, String> {
    let (meetings, warnings) = read_meetings(paths)?;
    if meetings.is_empty() {
        return Err("No Granola meetings found in those files.".to_string());
    }
    import_meetings(meetings, warnings, vault_root, margins_dir, options)
}

pub fn import_meetings(
    meetings: Vec<Meeting>,
    mut warnings: Vec<String>,
    vault_root: &Path,
    margins_dir: &Path,
    options: &GranolaImportOptions,
) -> Result<GranolaImportResult, String> {
    let notes_dir = vault_root.join(clean_folder(&options.notes_folder));
    let people_dir = vault_root.join(clean_folder(&options.people_folder));
    let org_dir = vault_root.join(clean_folder(&options.organizations_folder));
    let transcript_dir = vault_root.join(clean_folder(&options.transcripts_folder));
    std::fs::create_dir_all(&notes_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&people_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&org_dir).map_err(|e| e.to_string())?;
    if options.write_transcript_files {
        std::fs::create_dir_all(&transcript_dir).map_err(|e| e.to_string())?;
    }

    let mut note_paths = Vec::new();
    let mut transcript_paths = Vec::new();
    let mut session_ids = Vec::new();
    let mut created_people = BTreeSet::new();
    let mut created_orgs = BTreeSet::new();
    let mut skipped_already_imported = 0usize;
    let mut updated_count = 0usize;
    let mut existing_by_id = existing_granola_notes(&notes_dir);

    for mut meeting in meetings {
        let base_session_name = session_name_for(&meeting);
        let existing_path = meeting.id.as_ref().and_then(|id| existing_by_id.remove(id));
        if existing_path.is_none()
            && already_imported(margins_dir, &base_session_name).map_err(|e| e.to_string())?
        {
            skipped_already_imported += 1;
            continue;
        }

        let meeting_date = meeting_date(&meeting);
        for person in &meeting.people {
            let path = people_dir.join(format!("{}.md", file_stem(&person.name)));
            if !path.exists() {
                std::fs::write(&path, person_markdown(person, &meeting_date))
                    .map_err(|e| e.to_string())?;
                created_people.insert(path);
            }
        }
        for org in &meeting.organizations {
            let path = org_dir.join(format!("{}.md", file_stem(org)));
            if !path.exists() {
                std::fs::write(&path, organization_markdown(org, &meeting_date))
                    .map_err(|e| e.to_string())?;
                created_orgs.insert(path);
            }
        }

        let (note_path, stem, session_name) = match existing_path {
            Some(path) => {
                // Merge and replace in place, never downgrading: keep the richer
                // existing section when the incoming meeting lacks it (e.g. an MCP
                // summary-only sync after a file export already brought the transcript).
                let existing_content = std::fs::read_to_string(&path).ok();
                if let Some(content) = existing_content.as_deref() {
                    if meeting
                        .notes
                        .as_deref()
                        .map_or(true, |s| s.trim().is_empty())
                    {
                        meeting.notes = extract_section(content, "## Granola notes");
                    }
                    if meeting
                        .transcript
                        .as_deref()
                        .map_or(true, |s| s.trim().is_empty())
                    {
                        meeting.transcript = extract_section(content, "## Transcript");
                    }
                }
                let session_name = match existing_content
                    .as_deref()
                    .and_then(|c| frontmatter_value(c, "margins_session"))
                {
                    Some(name)
                        if session::session_exists(margins_dir, &name)
                            .map_err(|e| e.to_string())? =>
                    {
                        name
                    }
                    _ => unique_session_name(margins_dir, &base_session_name)
                        .map_err(|e| e.to_string())?,
                };
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| file_stem(&meeting.title));
                updated_count += 1;
                (path, stem, session_name)
            }
            None => {
                let stem =
                    unique_file_stem(&notes_dir, &format!("{meeting_date} {}", meeting.title));
                let path = notes_dir.join(format!("{stem}.md"));
                let session_name = unique_session_name(margins_dir, &base_session_name)
                    .map_err(|e| e.to_string())?;
                (path, stem, session_name)
            }
        };
        write_if_changed(
            &note_path,
            &meeting_markdown(&meeting, &session_name, options),
        )?;

        if options.write_transcript_files {
            if let Some(transcript) = meeting
                .transcript
                .as_deref()
                .filter(|s| !s.trim().is_empty())
            {
                let transcript_path = transcript_dir.join(format!("{stem} transcript.md"));
                write_if_changed(&transcript_path, &transcript_markdown(&meeting, transcript))?;
                transcript_paths.push(transcript_path.to_string_lossy().to_string());
            }
        }

        let start = meeting
            .created_at
            .as_deref()
            .and_then(parse_local_datetime)
            .unwrap_or_else(Local::now);
        if !session::session_exists(margins_dir, &session_name).map_err(|e| e.to_string())? {
            session::create_session(
                margins_dir,
                &session_name,
                &start,
                &note_path.to_string_lossy(),
            )
            .map_err(|e| e.to_string())?;
        }
        sync_session_metadata(margins_dir, &session_name, &meeting, Some(&note_path))
            .map_err(|e| e.to_string())?;
        session_ids.push(session_name);
        note_paths.push(note_path.to_string_lossy().to_string());
    }

    if skipped_already_imported > 0 {
        warnings.push(format!(
            "Skipped {skipped_already_imported} meetings already imported"
        ));
    }

    Ok(GranolaImportResult {
        imported_count: note_paths.len() - updated_count,
        updated_count,
        note_paths,
        people_created: created_people.len(),
        organizations_created: created_orgs.len(),
        transcript_paths,
        warnings,
        session_ids,
    })
}

fn write_if_changed(path: &Path, content: &str) -> Result<bool, String> {
    if std::fs::read(path).is_ok_and(|existing| existing == content.as_bytes()) {
        return Ok(false);
    }
    std::fs::write(path, content).map_err(|error| error.to_string())?;
    Ok(true)
}

fn sync_session_metadata(
    margins_dir: &Path,
    session_name: &str,
    meeting: &Meeting,
    vault_note_path: Option<&Path>,
) -> AnyResult<()> {
    let meta = session::get_session_meta(margins_dir, session_name)?;
    if meta.title.as_deref() != Some(meeting.title.as_str()) {
        session::set_title(margins_dir, session_name, Some(meeting.title.clone()))?;
    }
    let people = meeting
        .people
        .iter()
        .map(|person| person.name.clone())
        .collect::<Vec<_>>();
    if meta.people != people {
        session::set_people(margins_dir, session_name, people)?;
    }
    if let Some(path) = vault_note_path {
        let path = path.to_string_lossy();
        if meta.vault_note_path.as_deref() != Some(path.as_ref()) {
            session::set_vault_note_path(margins_dir, session_name, &path)?;
        }
    }
    Ok(())
}

pub fn validate_granola_file(path: &Path) -> AnyResult<usize> {
    let (meetings, warnings) =
        read_meetings(&[path.to_string_lossy().to_string()]).map_err(anyhow::Error::msg)?;
    if meetings.is_empty() {
        bail!(
            "{} does not match the supported Granola export structure{}",
            display_leaf(&path.to_string_lossy()),
            if warnings.is_empty() {
                ""
            } else {
                ": see import warnings"
            }
        );
    }
    Ok(meetings.len())
}

pub fn import_granola_to_margins(
    path: &Path,
    margins_dir: &Path,
) -> AnyResult<GranolaImportResult> {
    let path_string = path.to_string_lossy().to_string();
    let (meetings, warnings) =
        read_meetings(std::slice::from_ref(&path_string)).map_err(anyhow::Error::msg)?;
    if meetings.is_empty() {
        bail!("No Granola meetings found in {}.", path.display());
    }
    std::fs::create_dir_all(margins_dir)
        .with_context(|| format!("failed to create {}", margins_dir.display()))?;

    let mut note_paths = Vec::new();
    let mut transcript_paths = Vec::new();
    let mut session_ids = Vec::new();
    let mut updated_count = 0usize;
    let mut existing_by_id = existing_granola_notes(margins_dir);
    let mut existing_by_fallback = existing_granola_fallback_notes(margins_dir);
    for mut meeting in meetings {
        let base_session_name = session_name_for(&meeting);
        let existing_path = if let Some(id) = meeting.id.as_ref() {
            existing_by_id.remove(id)
        } else {
            existing_by_fallback
                .get_mut(&fallback_meeting_key(&meeting))
                .and_then(|paths| (!paths.is_empty()).then(|| paths.remove(0)))
        };
        let existing_content = existing_path
            .as_ref()
            .and_then(|note_path| std::fs::read_to_string(note_path).ok());
        if let Some(content) = existing_content.as_deref() {
            if meeting
                .notes
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                meeting.notes = extract_section(content, "## Granola notes");
            }
            if meeting
                .transcript
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                meeting.transcript = extract_section(content, "## Transcript");
            }
        }
        let existing_session_name = existing_content
            .as_deref()
            .and_then(|content| frontmatter_value(content, "margins_session"))
            .filter(|name| session::session_exists(margins_dir, name).unwrap_or(false));
        let session_name = match existing_session_name {
            Some(name) => name,
            None => unique_session_name(margins_dir, &base_session_name)?,
        };
        let start = meeting
            .created_at
            .as_deref()
            .and_then(parse_local_datetime)
            .unwrap_or_else(Local::now);
        let note_path =
            existing_path.unwrap_or_else(|| margins_dir.join(format!("{session_name}.md")));
        let capture_path = margins_dir.join(format!("{session_name}_capture_context.md"));
        let note_path_string = note_path.to_string_lossy().to_string();

        if !session::session_exists(margins_dir, &session_name)? {
            session::create_session(margins_dir, &session_name, &start, &note_path_string)
                .with_context(|| format!("failed to create session {session_name}"))?;
        }
        write_if_changed(
            &note_path,
            &meeting_markdown(
                &meeting,
                &session_name,
                &GranolaImportOptions::margins_default(),
            ),
        )
        .map_err(anyhow::Error::msg)?;
        write_if_changed(
            &capture_path,
            &capture_context_markdown(&session_name, path, &meeting),
        )
        .map_err(anyhow::Error::msg)?;
        sync_session_metadata(margins_dir, &session_name, &meeting, None)?;
        updated_count += usize::from(existing_content.is_some());

        session_ids.push(session_name);
        note_paths.push(note_path_string);
        transcript_paths.push(capture_path.to_string_lossy().to_string());
    }

    Ok(GranolaImportResult {
        imported_count: session_ids.len() - updated_count,
        updated_count,
        note_paths,
        people_created: 0,
        organizations_created: 0,
        transcript_paths,
        warnings,
        session_ids,
    })
}

pub fn import_granola_using_config(
    path: &Path,
    margins_dir: &Path,
) -> AnyResult<(GranolaImportResult, bool)> {
    import_granola_with_vault_target(path, margins_dir, None)
}

pub fn import_granola_with_vault_target(
    path: &Path,
    margins_dir: &Path,
    vault_target: Option<&GranolaVaultTarget>,
) -> AnyResult<(GranolaImportResult, bool)> {
    let config = publish::load_config(margins_dir)?;
    let paths = vec![path.to_string_lossy().to_string()];
    if let Some(config) = config.filter(|config| !config.vault.path.trim().is_empty()) {
        let options = GranolaImportOptions {
            notes_folder: folder_or_default(&config.vault.folder, "meetings"),
            people_folder: "people".to_string(),
            organizations_folder: "organizations".to_string(),
            transcripts_folder: "transcripts".to_string(),
            write_transcript_files: false,
        };
        let vault_root = PathBuf::from(expand_tilde(&config.vault.path));
        return import(&paths, &vault_root, margins_dir, &options)
            .map(|result| (result, true))
            .map_err(anyhow::Error::msg);
    }

    if let Some(target) = vault_target.filter(|target| !target.vault_root.as_os_str().is_empty()) {
        let options = GranolaImportOptions {
            notes_folder: folder_or_default(&target.notes_folder, "meetings"),
            people_folder: folder_or_default(&target.people_folder, "people"),
            organizations_folder: folder_or_default(&target.organizations_folder, "organizations"),
            transcripts_folder: "transcripts".to_string(),
            write_transcript_files: false,
        };
        return import(&paths, &target.vault_root, margins_dir, &options)
            .map(|result| (result, true))
            .map_err(anyhow::Error::msg);
    }

    import_granola_to_margins(path, margins_dir).map(|result| (result, false))
}

/// Folder options for automated (MCP) syncs: same defaults the survey suggests,
/// with transcripts embedded in the meeting notes.
pub fn default_options(
    vault_root: &Path,
    inbox_folder: &str,
    people_folder: &str,
) -> GranolaImportOptions {
    let candidates = folder_candidates(vault_root);
    GranolaImportOptions {
        notes_folder: folder_or_default(inbox_folder, "meetings"),
        people_folder: folder_or_default(people_folder, "people"),
        organizations_folder: choose_folder(&candidates, &["organizations", "orgs", "companies"])
            .unwrap_or_else(|| "organizations".to_string()),
        transcripts_folder: choose_folder(
            &candidates,
            &["transcripts", "meetings/transcripts", "raw transcripts"],
        )
        .unwrap_or_else(|| "transcripts".to_string()),
        write_transcript_files: false,
    }
}

impl GranolaImportOptions {
    fn margins_default() -> Self {
        Self {
            notes_folder: String::new(),
            people_folder: "people".to_string(),
            organizations_folder: "organizations".to_string(),
            transcripts_folder: "transcripts".to_string(),
            write_transcript_files: false,
        }
    }
}

fn read_meetings(paths: &[String]) -> Result<(Vec<Meeting>, Vec<String>), String> {
    let mut meetings = Vec::new();
    let mut warnings = Vec::new();
    for path in paths {
        let data =
            std::fs::read_to_string(path).map_err(|e| format!("Could not read {path}: {e}"))?;
        let trimmed = data.trim_start();
        let parsed = if trimmed.starts_with('{') || trimmed.starts_with('[') {
            meetings_from_json(&data)
        } else {
            meetings_from_csv(&data)
        };
        match parsed {
            Ok(mut found) if !found.is_empty() => meetings.append(&mut found),
            Ok(_) => warnings.push(format!("No meetings found in {}", display_leaf(path))),
            Err(e) => warnings.push(format!("{}: {e}", display_leaf(path))),
        }
    }
    Ok((meetings, warnings))
}

fn meetings_from_json(data: &str) -> Result<Vec<Meeting>, String> {
    let value: Value = match serde_json::from_str(data) {
        Ok(value) => value,
        Err(whole_file_error) => return meetings_from_json_lines(data, whole_file_error),
    };
    let mut out = Vec::new();
    collect_meetings(&value, &mut out);
    Ok(out)
}

fn meetings_from_json_lines(
    data: &str,
    whole_file_error: serde_json::Error,
) -> Result<Vec<Meeting>, String> {
    let mut out = Vec::new();
    let mut parsed_any = false;
    for (idx, line) in data.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(trimmed).map_err(|e| {
            format!(
                "JSON parse failed ({whole_file_error}); line {}: {e}",
                idx + 1
            )
        })?;
        parsed_any = true;
        collect_meetings(&value, &mut out);
    }
    if parsed_any {
        Ok(out)
    } else {
        Err(whole_file_error.to_string())
    }
}

fn collect_meetings(value: &Value, out: &mut Vec<Meeting>) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_meetings(item, out);
            }
        }
        Value::Object(map) => {
            if looks_like_meeting(value) {
                out.push(meeting_from_value(value));
                return;
            }
            for key in ["meetings", "documents", "data", "items", "results"] {
                if let Some(child) = map.get(key) {
                    collect_meetings(child, out);
                }
            }
        }
        _ => {}
    }
}

fn looks_like_meeting(value: &Value) -> bool {
    text_field(value, &["title", "name"]).is_some()
        && (text_field(value, &["transcript", "summary", "notes", "enhanced_notes"]).is_some()
            || value.pointer("/people").is_some()
            || value.pointer("/attendees").is_some()
            || value.pointer("/organizations").is_some()
            || value.pointer("/companies").is_some()
            || value.pointer("/entities").is_some())
}

pub fn meeting_from_value(value: &Value) -> Meeting {
    let title = text_field(value, &["title", "name"])
        .unwrap_or_else(|| "Untitled Granola meeting".to_string());
    let id = text_field(value, &["id", "document_id", "meeting_id"]);
    let created_at = text_field(
        value,
        &[
            "created_at",
            "createdAt",
            "started_at",
            "start_time",
            "date",
        ],
    );
    let notes = markdownish_field(
        value,
        &["enhanced_notes", "summary", "notes", "markdown", "content"],
    );
    let transcript = transcript_from_value(value);
    let people = people_from_value(value);
    let organizations = organizations_from_value(value, &people);
    Meeting {
        id,
        title,
        created_at,
        notes,
        transcript,
        people,
        organizations,
        plan_gated_transcript: false,
    }
}

fn meetings_from_csv(data: &str) -> Result<Vec<Meeting>, String> {
    let rows = parse_csv(data);
    if rows.len() < 2 {
        return Ok(Vec::new());
    }
    let headers: Vec<String> = rows[0].iter().map(|h| normalize_key(h)).collect();
    let mut out = Vec::new();
    for row in rows.into_iter().skip(1) {
        let mut map = BTreeMap::new();
        for (idx, value) in row.into_iter().enumerate() {
            if let Some(key) = headers.get(idx) {
                map.insert(key.clone(), value);
            }
        }
        let title = csv_get(&map, &["title", "name"])
            .unwrap_or_else(|| "Untitled Granola meeting".to_string());
        let created_at = csv_get(&map, &["created_at", "createdat", "date", "started_at"]);
        let notes = csv_get(&map, &["summary", "notes", "note"]);
        let transcript = csv_get(&map, &["transcript", "transcription"]);
        let people = csv_people(&map);
        let organizations = organizations_from_csv(&map, &people);
        out.push(Meeting {
            id: csv_get(&map, &["id", "document_id", "meeting_id"]),
            title,
            created_at,
            notes,
            transcript,
            people,
            organizations,
            plan_gated_transcript: false,
        });
    }
    Ok(out)
}

fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn markdownish_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(s) = value.get(*key).and_then(Value::as_str) {
            let s = s.trim();
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
        if let Some(v) = value.get(*key).filter(|v| v.is_object() || v.is_array()) {
            return Some(render_json_markdown(v));
        }
    }
    None
}

fn transcript_from_value(value: &Value) -> Option<String> {
    if let Some(s) = text_field(value, &["transcript", "transcription"]) {
        return Some(s);
    }
    for key in [
        "transcript",
        "transcription",
        "transcript_items",
        "transcriptEntries",
        "transcript_segments",
        "transcripts",
    ] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            let lines: Vec<String> = items.iter().filter_map(transcript_line).collect();
            if !lines.is_empty() {
                return Some(lines.join("\n\n"));
            }
        }
    }
    None
}

fn transcript_line(value: &Value) -> Option<String> {
    let text = text_field(value, &["text", "content", "utterance"])?;
    let speaker = speaker_label(value);
    let ts = text_field(value, &["timestamp", "start", "start_time"])
        .map(|s| format!("[{s}] "))
        .unwrap_or_default();
    Some(format!("{ts}**{speaker}:** {text}"))
}

fn speaker_label(value: &Value) -> String {
    let raw = text_field(value, &["speaker", "speaker_name", "name", "source"])
        .or_else(|| {
            value.get("speaker").and_then(|speaker| {
                text_field(speaker, &["name", "diarization_label", "source"]).or_else(|| {
                    let source = speaker.get("source").and_then(Value::as_str)?;
                    let label = speaker.get("diarization_label").and_then(Value::as_str);
                    Some(match label {
                        Some(label) if !label.trim().is_empty() => format!("{source} {label}"),
                        _ => source.to_string(),
                    })
                })
            })
        })
        .unwrap_or_else(|| "Speaker".to_string());
    // Granola labels raw audio channels rather than names: the mic channel is
    // the note-taker, the system channel is everyone else on the call.
    match raw.to_ascii_lowercase().as_str() {
        "microphone" | "mic" => "Me".to_string(),
        "system" | "speaker" => "Them".to_string(),
        _ => raw,
    }
}

/// Render transcript segments as coalesced speaker turns: consecutive segments
/// from the same speaker merge into one paragraph-per-turn block.
pub fn transcript_turns(items: &[Value]) -> Option<String> {
    let mut turns: Vec<(String, Vec<String>)> = Vec::new();
    for item in items {
        let Some(text) = text_field(item, &["text", "content", "utterance"]) else {
            continue;
        };
        let speaker = speaker_label(item);
        match turns.last_mut() {
            Some((last, texts)) if *last == speaker => texts.push(text),
            _ => turns.push((speaker, vec![text])),
        }
    }
    if turns.is_empty() {
        return None;
    }
    Some(
        turns
            .iter()
            .map(|(speaker, texts)| format!("**{speaker}:** {}", texts.join(" ")))
            .collect::<Vec<_>>()
            .join("\n\n"),
    )
}

fn people_from_value(value: &Value) -> Vec<Person> {
    let mut out = Vec::new();
    if let Some(people) = value.get("people") {
        if let Some(creator) = people.get("creator") {
            push_person(&mut out, creator);
        }
        if let Some(attendees) = people.get("attendees").and_then(Value::as_array) {
            for attendee in attendees {
                push_person(&mut out, attendee);
            }
        }
    }
    for key in ["attendees", "participants", "people"] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            for item in items {
                push_person(&mut out, item);
            }
        }
    }
    dedupe_people(out)
}

fn push_person(out: &mut Vec<Person>, value: &Value) {
    if let Some(name) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        out.push(Person {
            name: name.to_string(),
            email: None,
            organizations: Vec::new(),
        });
        return;
    }
    let Some(obj) = value.as_object() else {
        return;
    };
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| obj.get("display_name").and_then(Value::as_str))
        .or_else(|| obj.get("email").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(name) = name {
        out.push(Person {
            name: name.to_string(),
            email: obj.get("email").and_then(Value::as_str).map(str::to_string),
            organizations: org_fields_from_object(obj),
        });
    }
}

fn dedupe_people(people: Vec<Person>) -> Vec<Person> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for person in people {
        let key = person
            .email
            .as_deref()
            .unwrap_or(&person.name)
            .to_ascii_lowercase();
        if seen.insert(key) {
            out.push(person);
        }
    }
    out
}

fn csv_people(map: &BTreeMap<String, String>) -> Vec<Person> {
    let raw = csv_get(map, &["people", "attendees", "participants"]).unwrap_or_default();
    raw.split([';', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|name| Person {
            name: name.to_string(),
            email: name.contains('@').then(|| name.to_string()),
            organizations: Vec::new(),
        })
        .collect()
}

fn org_fields_from_object(obj: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["company", "company_name", "organization", "org"] {
        if let Some(org) = obj
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push(org.to_string());
        }
        if let Some(org_obj) = obj.get(key).and_then(Value::as_object) {
            if let Some(name) = org_obj
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                out.push(name.to_string());
            }
        }
    }
    dedupe_strings_case_insensitive(out)
}

fn organizations_from_csv(map: &BTreeMap<String, String>, people: &[Person]) -> Vec<String> {
    let mut out = organizations_from_people(people);
    for key in [
        "organizations",
        "organization",
        "companies",
        "company",
        "org",
    ] {
        if let Some(raw) = csv_get(map, &[key]) {
            out.extend(split_org_list(&raw));
        }
    }
    dedupe_strings_case_insensitive(out)
}

fn organizations_from_value(value: &Value, people: &[Person]) -> Vec<String> {
    let mut out = organizations_from_people(people);
    for key in ["organizations", "companies", "entities"] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            for item in items {
                if let Some(name) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    out.push(name.to_string());
                } else if let Some(name) =
                    text_field(item, &["name", "title", "company", "organization", "org"])
                {
                    out.push(name);
                }
            }
        }
    }
    for key in ["company", "company_name", "organization", "org"] {
        if let Some(name) = text_field(value, &[key]) {
            out.push(name);
        }
    }
    dedupe_strings_case_insensitive(out)
}

fn organizations_from_people(people: &[Person]) -> Vec<String> {
    let mut out = Vec::new();
    for person in people {
        out.extend(person.organizations.iter().cloned());
        if person.organizations.is_empty() {
            if let Some(domain) = person.email.as_deref().and_then(|e| e.split('@').nth(1)) {
                let root = domain.split('.').next().unwrap_or_default();
                if !matches!(
                    root,
                    "gmail"
                        | "googlemail"
                        | "icloud"
                        | "me"
                        | "outlook"
                        | "hotmail"
                        | "yahoo"
                        | "proton"
                        | "hey"
                ) && !root.is_empty()
                {
                    out.push(title_case(root));
                }
            }
        }
    }
    dedupe_strings_case_insensitive(out)
}

fn split_org_list(raw: &str) -> Vec<String> {
    raw.split([';', ','])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn dedupe_strings_case_insensitive(values: Vec<String>) -> Vec<String> {
    let mut by_key = BTreeMap::new();
    for value in values {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            by_key
                .entry(trimmed.to_ascii_lowercase())
                .or_insert_with(|| trimmed.to_string());
        }
    }
    by_key.into_values().collect()
}

fn meeting_markdown(
    meeting: &Meeting,
    session_name: &str,
    options: &GranolaImportOptions,
) -> String {
    let created_date = meeting_date(meeting);
    let people_links: Vec<String> = meeting
        .people
        .iter()
        .map(|p| format!("[[{}]]", file_stem(&p.name)))
        .collect();
    let org_links: Vec<String> = meeting
        .organizations
        .iter()
        .map(|o| format!("[[{}]]", file_stem(o)))
        .collect();
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("created: '[[{}]]'\n", yaml_escape(&created_date)));
    out.push_str(&format!("margins_session: {}\n", yaml_escape(session_name)));
    out.push_str(&format!(
        "title: '{}'\n",
        yaml_single_quote_escape(&meeting.title)
    ));
    out.push_str("source: granola\n");
    if let Some(id) = &meeting.id {
        out.push_str(&format!("granola_id: '{}'\n", yaml_single_quote_escape(id)));
    }
    yaml_list(&mut out, "people", &people_links);
    yaml_list(&mut out, "organizations", &org_links);
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", meeting.title));
    if !people_links.is_empty() {
        out.push_str(&format!("People: {}\n\n", people_links.join(", ")));
    }
    if !org_links.is_empty() {
        out.push_str(&format!("Organizations: {}\n\n", org_links.join(", ")));
    }
    out.push_str("## Granola notes\n\n");
    out.push_str(
        meeting
            .notes
            .as_deref()
            .unwrap_or("_No Granola summary was present in the export._"),
    );
    out.push_str("\n\n## Transcript\n\n");
    out.push_str(
        meeting
            .transcript
            .as_deref()
            .unwrap_or("_No transcript was present in the export._"),
    );
    if options.write_transcript_files {
        out.push_str("\n");
    }
    out
}

fn transcript_markdown(meeting: &Meeting, transcript: &str) -> String {
    format!(
        "---\ntitle: \"{} transcript\"\nsource: granola\n---\n\n# {} transcript\n\n{}",
        yaml_escape(&meeting.title),
        meeting.title,
        transcript
    )
}

fn person_markdown(person: &Person, created_date: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("type: person\n");
    out.push_str("tags: [people]\n");
    out.push_str(&format!("created: {}\n", yaml_escape(created_date)));
    out.push_str("source: granola\n");
    if let Some(email) = &person.email {
        out.push_str(&format!("email: {}\n", yaml_escape(email)));
    }
    let organizations = person_organizations(person);
    if !organizations.is_empty() {
        out.push_str("organizations:\n");
        for org in &organizations {
            out.push_str(&format!("- {}\n", yaml_escape(org)));
        }
    }
    out.push_str("---\n");
    out.push_str(&format!("# {}\n", person.name));
    out
}

fn person_organizations(person: &Person) -> Vec<String> {
    organizations_from_people(std::slice::from_ref(person))
}

fn organization_markdown(org: &str, created_date: &str) -> String {
    format!(
        "---\ntype: organization\ntags: [organizations]\ncreated: {}\nsource: granola\n---\n# {}\n",
        yaml_escape(created_date),
        org
    )
}

fn folder_candidates(vault_root: &Path) -> Vec<String> {
    let mut out = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(vault_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if !name.starts_with('.') {
                        out.insert(name.to_string());
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

fn choose_folder(candidates: &[String], names: &[&str]) -> Option<String> {
    for wanted in names {
        if let Some(found) = candidates.iter().find(|c| c.eq_ignore_ascii_case(wanted)) {
            return Some(found.clone());
        }
    }
    None
}

fn folder_or_default(value: &str, fallback: &str) -> String {
    let cleaned = clean_folder(value);
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

fn clean_folder(value: &str) -> String {
    value.trim().trim_matches('/').replace('\\', "/")
}

fn file_stem(value: &str) -> String {
    let stem = value
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*') {
                '-'
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    if stem.is_empty() {
        "Untitled".to_string()
    } else {
        stem
    }
}

fn existing_granola_notes(notes_dir: &Path) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(notes_dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(id) = frontmatter_value(&content, "granola_id") {
            out.entry(id).or_insert(path);
        }
    }
    out
}

fn existing_granola_fallback_notes(notes_dir: &Path) -> BTreeMap<String, Vec<PathBuf>> {
    let mut out = BTreeMap::<String, Vec<PathBuf>>::new();
    let Ok(entries) = std::fs::read_dir(notes_dir) else {
        return out;
    };
    let mut paths = entries
        .flatten()
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if frontmatter_value(&content, "source").as_deref() != Some("granola")
            || frontmatter_value(&content, "granola_id").is_some()
        {
            continue;
        }
        let Some(title) = frontmatter_value(&content, "title") else {
            continue;
        };
        let Some(created) = frontmatter_value(&content, "created") else {
            continue;
        };
        let created = created
            .strip_prefix("[[")
            .and_then(|value| value.strip_suffix("]]"))
            .unwrap_or(&created);
        out.entry(format!("{created}|{title}"))
            .or_default()
            .push(path);
    }
    out
}

fn fallback_meeting_key(meeting: &Meeting) -> String {
    format!("{}|{}", meeting_date(meeting), meeting.title)
}

fn frontmatter_value(content: &str, key: &str) -> Option<String> {
    let rest = content.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let prefix = format!("{key}:");
    for line in rest[..end].lines() {
        if let Some(value) = line.strip_prefix(&prefix) {
            let value = value.trim();
            let value = if let Some(inner) =
                value.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))
            {
                inner.replace("''", "'")
            } else if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                inner.replace("\\\"", "\"")
            } else {
                value.to_string()
            };
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

/// Body of a `## heading` section, or None if missing/empty/placeholder.
fn extract_section(content: &str, heading: &str) -> Option<String> {
    let marker = format!("\n{heading}\n");
    let start = content.find(&marker)? + marker.len();
    let rest = &content[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    let body = rest[..end].trim();
    if body.is_empty()
        || body == "_No Granola summary was present in the export._"
        || body == "_No transcript was present in the export._"
        || body == "_Transcripts aren't available over Granola's API on your plan — drag a Granola export (JSON) onto the sidebar to import them._"
    {
        return None;
    }
    Some(body.to_string())
}

fn unique_file_stem(dir: &Path, desired: &str) -> String {
    let base = file_stem(desired);
    for idx in 0..1000 {
        let candidate = if idx == 0 {
            base.clone()
        } else {
            format!("{base} {idx}")
        };
        if !dir.join(format!("{candidate}.md")).exists() {
            return candidate;
        }
    }
    format!("{base} 1000")
}

fn unique_session_name(margins_dir: &Path, desired: &str) -> AnyResult<String> {
    for idx in 0..1000 {
        let candidate = if idx == 0 {
            desired.to_string()
        } else {
            format!("{desired}-{idx}")
        };
        if !session::session_exists(margins_dir, &candidate)? {
            return Ok(candidate);
        }
    }
    Ok(format!("{desired}-1000"))
}

fn already_imported(margins_dir: &Path, session_name: &str) -> AnyResult<bool> {
    if !session::session_exists(margins_dir, session_name)? {
        return Ok(false);
    }
    let meta = session::get_session_meta(margins_dir, session_name)?;
    Ok(meta
        .vault_note_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .map(|path| Path::new(path).exists())
        .unwrap_or(false))
}

fn session_name_for(meeting: &Meeting) -> String {
    let source = meeting.id.as_deref().unwrap_or(&meeting.title);
    let slug = source
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let mut out = String::new();
    let mut prev_dash = true;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(c);
            prev_dash = false;
        }
        if out.len() >= 80 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "granola-import".to_string()
    } else {
        out
    }
}

fn meeting_date(meeting: &Meeting) -> String {
    meeting
        .created_at
        .as_deref()
        .and_then(date_prefix)
        .unwrap_or_else(|| Local::now().format("%Y-%m-%d").to_string())
}

fn parse_local_datetime(value: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Local))
}

fn date_prefix(value: &str) -> Option<String> {
    parse_local_datetime(value)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .or_else(|| {
            value
                .get(0..10)
                .map(str::to_string)
                .filter(|s| s.chars().filter(|c| *c == '-').count() == 2)
        })
}

fn render_json_markdown(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

fn parse_csv(data: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = data.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                row.push(field.trim().to_string());
                field.clear();
            }
            '\n' if !quoted => {
                row.push(field.trim().to_string());
                field.clear();
                if row.iter().any(|c| !c.is_empty()) {
                    rows.push(std::mem::take(&mut row));
                } else {
                    row.clear();
                }
            }
            '\r' if !quoted => {}
            _ => field.push(ch),
        }
    }
    row.push(field.trim().to_string());
    if row.iter().any(|c| !c.is_empty()) {
        rows.push(row);
    }
    rows
}

fn normalize_key(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

fn csv_get(map: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| map.get(&normalize_key(key)))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn yaml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn yaml_single_quote_escape(value: &str) -> String {
    value.replace('\'', "''")
}

fn yaml_list(out: &mut String, key: &str, values: &[String]) {
    out.push_str(key);
    out.push_str(":\n");
    for value in values {
        out.push_str(&format!("  - \"{}\"\n", yaml_escape(value)));
    }
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn capture_context_markdown(session_name: &str, source_path: &Path, meeting: &Meeting) -> String {
    let mut out = format!(
        "# Granola import capture context\n\nSession: `{}`\nSource: `{}`\n\n",
        session_name,
        source_path.display()
    );
    out.push_str("## Meeting\n\n");
    out.push_str(&format!("- title: {}\n", meeting.title));
    if let Some(id) = &meeting.id {
        out.push_str(&format!("- granola_id: {id}\n"));
    }
    if let Some(created_at) = &meeting.created_at {
        out.push_str(&format!("- created_at: {created_at}\n"));
    }
    out.push_str("\n## Transcript\n\n");
    out.push_str(
        meeting
            .transcript
            .as_deref()
            .unwrap_or("_No transcript was present in the export._"),
    );
    out.push('\n');
    out
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    path.to_string()
}

fn display_leaf(path: &str) -> String {
    PathBuf::from(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_core::{SessionId, SessionRepository};
    use margins_store::SqliteSessionRepository;

    const GRANOLA_JSON: &str = r#"{
  "documents": [
    {
      "id": "grn_001",
      "title": "Pilot scope review",
      "created_at": "2026-06-24T15:30:00Z",
      "enhanced_notes": "Discussed launch scope and owner handoffs.",
      "people": {
        "creator": { "name": "Maya Patel", "email": "maya@gmail.com" },
        "attendees": [
          { "name": "Alex Chen", "email": "alex@gmail.com", "company": "Acme Labs" },
          { "name": "Bob Example", "email": "bob@example.com" },
          { "name": "Dana/Lee", "email": "dana@icloud.com" }
        ]
      },
      "entities": [
        { "name": "Northstar", "type": "company" }
      ],
      "organizations": [
        { "name": "Acme Labs" }
      ],
      "transcript_segments": [
        { "speaker": { "name": "Maya Patel" }, "start": "00:00", "text": "Welcome to the pilot review." },
        { "speaker_name": "Alex Chen", "start": "00:17", "text": "Acme Labs can own the rollout." }
      ]
    }
  ]
}"#;

    const GRANOLA_JSONL: &str = r#"{"documents":[{"id":"line_1","title":"Line one","created_at":"2026-06-25T10:00:00Z","notes":"First note","people":{"attendees":[{"name":"Riley Stone","email":"riley@contoso.com"}]}}]}
{"id":"line_2","title":"Line two","created_at":"2026-06-26T11:00:00Z","summary":"Second note","attendees":["Sam River"],"companies":[{"name":"River Works"}]}
"#;

    const GRANOLA_CSV: &str = "id,title,created_at,notes,transcript,people,company\ncsv_1,CSV sync,2026-06-27T12:00:00Z,CSV notes,CSV transcript,Casey One;casey@widgets.com,WidgetCo\n";

    const ORG_PRECEDENCE_JSON: &str = r#"{
  "documents": [
    {
      "id": "org_precedence",
      "title": "Org precedence",
      "created_at": "2026-06-28T09:00:00Z",
      "notes": "Explicit company should suppress email-domain inference.",
      "people": {
        "attendees": [
          { "name": "Ada Lovelace", "email": "ada@analytical.co", "company_name": "Analytical Engines" }
        ]
      }
    }
  ]
}"#;

    fn options() -> GranolaImportOptions {
        GranolaImportOptions {
            notes_folder: "meetings".to_string(),
            people_folder: "people".to_string(),
            organizations_folder: "organizations".to_string(),
            transcripts_folder: "transcripts".to_string(),
            write_transcript_files: false,
        }
    }

    fn write_fixture(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn import_fixture(root: &Path, fixture: &Path) -> GranolaImportResult {
        let margins_dir = root.join(".margins");
        import(
            &[fixture.to_string_lossy().to_string()],
            root,
            &margins_dir,
            &options(),
        )
        .unwrap()
    }

    #[test]
    fn json_import_writes_enzyme_frontmatter_and_entities() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);

        let result = import_fixture(dir.path(), &fixture);

        assert_eq!(result.imported_count, 1);
        assert_eq!(result.people_created, 4);
        assert_eq!(result.organizations_created, 3);
        let note = std::fs::read_to_string(dir.path().join("meetings/Pilot scope review.md"))
            .or_else(|_| {
                std::fs::read_to_string(
                    dir.path().join("meetings/2026-06-24 Pilot scope review.md"),
                )
            })
            .unwrap();
        let expected_frontmatter = r#"---
created: '[[2026-06-24]]'
margins_session: grn-001
title: 'Pilot scope review'
source: granola
granola_id: 'grn_001'
people:
  - "[[Maya Patel]]"
  - "[[Alex Chen]]"
  - "[[Bob Example]]"
  - "[[Dana-Lee]]"
organizations:
  - "[[Acme Labs]]"
  - "[[Example]]"
  - "[[Northstar]]"
---
"#;
        assert!(note.starts_with(expected_frontmatter), "{note}");
        assert!(note.contains("## Transcript\n\n[00:00] **Maya Patel:** Welcome to the pilot review.\n\n[00:17] **Alex Chen:** Acme Labs can own the rollout."));

        let person = std::fs::read_to_string(dir.path().join("people/Alex Chen.md")).unwrap();
        assert_eq!(
            person,
            "---\ntype: person\ntags: [people]\ncreated: 2026-06-24\nsource: granola\nemail: alex@gmail.com\norganizations:\n- Acme Labs\n---\n# Alex Chen\n"
        );
        assert!(dir.path().join("people/Dana-Lee.md").exists());
        assert!(dir.path().join("organizations/Example.md").exists());
        assert!(dir.path().join("organizations/Acme Labs.md").exists());
        assert!(dir.path().join("organizations/Northstar.md").exists());
    }

    #[test]
    fn jsonl_parses_multiple_line_delimited_meetings() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.jsonl", GRANOLA_JSONL);

        let result = import_fixture(dir.path(), &fixture);

        assert_eq!(result.imported_count, 2);
        assert!(dir.path().join("meetings/2026-06-25 Line one.md").exists());
        assert!(dir.path().join("meetings/2026-06-26 Line two.md").exists());
        assert!(dir.path().join("organizations/Contoso.md").exists());
        assert!(dir.path().join("organizations/River Works.md").exists());
    }

    #[test]
    fn csv_import_extracts_people_and_organizations() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.csv", GRANOLA_CSV);

        let result = import_fixture(dir.path(), &fixture);

        assert_eq!(result.imported_count, 1);
        let note =
            std::fs::read_to_string(dir.path().join("meetings/2026-06-27 CSV sync.md")).unwrap();
        assert!(note.contains("granola_id: 'csv_1'"));
        assert!(note.contains("  - \"[[WidgetCo]]\""));
        assert!(note.contains("  - \"[[Widgets]]\""));
    }

    #[test]
    fn explicit_person_org_suppresses_email_domain_inference() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "org-precedence.json", ORG_PRECEDENCE_JSON);

        let result = import_fixture(dir.path(), &fixture);

        assert_eq!(result.imported_count, 1);
        let note =
            std::fs::read_to_string(dir.path().join("meetings/2026-06-28 Org precedence.md"))
                .unwrap();
        assert!(note.contains("  - \"[[Analytical Engines]]\""));
        assert!(!note.contains("Analytical]]"));
        assert!(dir
            .path()
            .join("organizations/Analytical Engines.md")
            .exists());
        assert!(!dir.path().join("organizations/Analytical.md").exists());
        let person = std::fs::read_to_string(dir.path().join("people/Ada Lovelace.md")).unwrap();
        assert!(person.contains("organizations:\n- Analytical Engines\n"));
        assert!(!person.contains("- Analytical\n"));
    }

    #[test]
    fn person_notes_are_not_overwritten_on_reimport() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);

        import_fixture(dir.path(), &fixture);
        let person_path = dir.path().join("people/Alex Chen.md");
        std::fs::write(&person_path, "custom person note\n").unwrap();
        let second = import_fixture(dir.path(), &fixture);

        assert_eq!(second.people_created, 0);
        assert_eq!(
            std::fs::read_to_string(person_path).unwrap(),
            "custom person note\n"
        );
    }

    #[test]
    fn import_is_byte_deterministic_for_same_export_and_empty_vault() {
        let fixture_dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(fixture_dir.path(), "granola.json", GRANOLA_JSON);
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();

        import_fixture(a.path(), &fixture);
        import_fixture(b.path(), &fixture);

        let note_a =
            std::fs::read(a.path().join("meetings/2026-06-24 Pilot scope review.md")).unwrap();
        let note_b =
            std::fs::read(b.path().join("meetings/2026-06-24 Pilot scope review.md")).unwrap();
        assert_eq!(note_a, note_b);
    }

    #[test]
    fn reimport_replaces_existing_meetings_by_granola_id_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.jsonl", GRANOLA_JSONL);

        let first = import_fixture(dir.path(), &fixture);
        let meeting_count = std::fs::read_dir(dir.path().join("meetings"))
            .unwrap()
            .count();
        let people_count = std::fs::read_dir(dir.path().join("people"))
            .unwrap()
            .count();
        let org_count = std::fs::read_dir(dir.path().join("organizations"))
            .unwrap()
            .count();
        let second = import_fixture(dir.path(), &fixture);

        assert_eq!(first.imported_count, 2);
        assert_eq!(second.imported_count, 0);
        assert_eq!(second.updated_count, 2);
        assert_eq!(second.people_created, 0);
        assert_eq!(second.organizations_created, 0);
        assert_eq!(second.note_paths.len(), 2);
        assert_eq!(second.session_ids.len(), 2);
        assert_eq!(first.session_ids, second.session_ids);
        assert_eq!(
            std::fs::read_dir(dir.path().join("meetings"))
                .unwrap()
                .count(),
            meeting_count
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("people"))
                .unwrap()
                .count(),
            people_count
        );
        assert_eq!(
            std::fs::read_dir(dir.path().join("organizations"))
                .unwrap()
                .count(),
            org_count
        );
        assert!(!dir
            .path()
            .join("meetings/2026-06-25 Line one 1.md")
            .exists());
        assert!(!dir
            .path()
            .join("meetings/2026-06-26 Line two 1.md")
            .exists());
    }

    #[test]
    fn identical_reimport_is_a_store_revision_noop() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);
        let margins_dir = dir.path().join(".margins");
        let repository = SqliteSessionRepository::open(&margins_dir).unwrap();

        let first = import_fixture(dir.path(), &fixture);
        let id = SessionId::from(first.session_ids[0].clone());
        let first_revision = repository.get(&id).unwrap().unwrap().revision;
        let note = std::fs::read(&first.note_paths[0]).unwrap();
        let second = import_fixture(dir.path(), &fixture);
        let second_revision = repository.get(&id).unwrap().unwrap().revision;

        assert_eq!(second.imported_count, 0);
        assert_eq!(second.updated_count, 1);
        assert_eq!(first.session_ids, second.session_ids);
        assert_eq!(second_revision, first_revision);
        assert_eq!(std::fs::read(&second.note_paths[0]).unwrap(), note);
    }

    #[test]
    fn margins_fallback_import_is_idempotent_with_and_without_granola_id() {
        for (name, fixture_body) in [
            ("with-id.json", GRANOLA_JSON),
            (
                "without-id.jsonl",
                r#"{"title":"Anon meeting","created_at":"2026-06-29T09:00:00Z","notes":"Notes"}
"#,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let fixture = write_fixture(dir.path(), name, fixture_body);
            let margins_dir = dir.path().join(".margins");
            let first = import_granola_to_margins(&fixture, &margins_dir).unwrap();
            let second = import_granola_to_margins(&fixture, &margins_dir).unwrap();

            assert_eq!(first.imported_count, 1);
            assert_eq!(second.imported_count, 0);
            assert_eq!(second.updated_count, 1);
            assert_eq!(first.session_ids, second.session_ids);
            assert_eq!(first.note_paths, second.note_paths);
            assert_eq!(first.transcript_paths, second.transcript_paths);
            assert_eq!(session::list_sessions(&margins_dir).unwrap().len(), 1);
        }
    }

    #[test]
    fn reimport_by_granola_id_never_downgrades_sections() {
        let dir = tempfile::tempdir().unwrap();
        let full = write_fixture(
            dir.path(),
            "full.jsonl",
            r#"{"id":"doc_9","title":"Weekly sync","created_at":"2026-06-30T10:00:00Z","notes":"Original summary","transcript":"**Me:** hello there"}
"#,
        );
        import_fixture(dir.path(), &full);

        let summary_only = write_fixture(
            dir.path(),
            "summary.jsonl",
            r#"{"id":"doc_9","title":"Weekly sync","created_at":"2026-06-30T10:00:00Z","notes":"Fresher summary"}
"#,
        );
        let second = import_fixture(dir.path(), &summary_only);

        assert_eq!(second.imported_count, 0);
        assert_eq!(second.updated_count, 1);
        let note_path = dir.path().join("meetings/2026-06-30 Weekly sync.md");
        let note = std::fs::read_to_string(&note_path).unwrap();
        assert!(note.contains("Fresher summary"));
        assert!(!note.contains("Original summary"));
        assert!(note.contains("**Me:** hello there"));
        assert!(!note.contains("_No transcript was present in the export._"));
    }

    #[test]
    fn reimport_without_granola_id_skips_existing_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(
            dir.path(),
            "no-id.jsonl",
            r#"{"title":"Anon meeting","created_at":"2026-06-29T09:00:00Z","notes":"Notes"}
"#,
        );
        let first = import_fixture(dir.path(), &fixture);
        let second = import_fixture(dir.path(), &fixture);

        assert_eq!(first.imported_count, 1);
        assert_eq!(second.imported_count, 0);
        assert_eq!(second.updated_count, 0);
        assert!(second
            .warnings
            .iter()
            .any(|warning| warning == "Skipped 1 meetings already imported"));
        assert!(!dir
            .path()
            .join("meetings/2026-06-29 Anon meeting 1.md")
            .exists());
    }

    #[test]
    fn config_import_writes_full_vault_import() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);
        let margins_dir = dir.path().join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        std::fs::write(
            margins_dir.join("config.toml"),
            format!(
                "[vault]\npath = {:?}\nfolder = \"calls\"\n",
                dir.path().to_string_lossy()
            ),
        )
        .unwrap();

        let (result, used_vault) = import_granola_using_config(&fixture, &margins_dir).unwrap();

        assert!(used_vault);
        assert_eq!(result.imported_count, 1);
        assert!(dir
            .path()
            .join("calls/2026-06-24 Pilot scope review.md")
            .exists());
        assert!(dir.path().join("people/Alex Chen.md").exists());
        assert!(dir.path().join("organizations/Acme Labs.md").exists());
        let meta = session::get_session_meta(&margins_dir, "grn-001").unwrap();
        assert_eq!(
            meta.vault_note_path.as_deref(),
            Some(
                dir.path()
                    .join("calls/2026-06-24 Pilot scope review.md")
                    .to_string_lossy()
                    .as_ref()
            )
        );
    }

    #[test]
    fn project_target_import_writes_full_vault_import_without_config() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);
        let vault = dir.path().join("vault");
        let margins_dir = dir.path().join("work/.margins");
        let target = GranolaVaultTarget {
            vault_root: vault.clone(),
            notes_folder: "calls".to_string(),
            people_folder: "contacts".to_string(),
            organizations_folder: "organizations".to_string(),
        };

        let (result, used_vault) =
            import_granola_with_vault_target(&fixture, &margins_dir, Some(&target)).unwrap();

        assert!(used_vault);
        assert_eq!(result.imported_count, 1);
        assert!(vault
            .join("calls/2026-06-24 Pilot scope review.md")
            .exists());
        assert!(vault.join("contacts/Alex Chen.md").exists());
        assert!(vault.join("organizations/Acme Labs.md").exists());
        let meta = session::get_session_meta(&margins_dir, "grn-001").unwrap();
        assert_eq!(
            meta.vault_note_path.as_deref(),
            Some(
                vault
                    .join("calls/2026-06-24 Pilot scope review.md")
                    .to_string_lossy()
                    .as_ref()
            )
        );
    }

    #[test]
    fn config_vault_takes_priority_over_project_target() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);
        let config_vault = dir.path().join("config-vault");
        let project_vault = dir.path().join("project-vault");
        let margins_dir = dir.path().join("work/.margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        std::fs::write(
            margins_dir.join("config.toml"),
            format!(
                "[vault]\npath = {:?}\nfolder = \"config-notes\"\n",
                config_vault.to_string_lossy()
            ),
        )
        .unwrap();
        let target = GranolaVaultTarget {
            vault_root: project_vault.clone(),
            notes_folder: "project-notes".to_string(),
            people_folder: "project-people".to_string(),
            organizations_folder: "project-orgs".to_string(),
        };

        let (result, used_vault) =
            import_granola_with_vault_target(&fixture, &margins_dir, Some(&target)).unwrap();

        assert!(used_vault);
        assert_eq!(result.imported_count, 1);
        assert!(config_vault
            .join("config-notes/2026-06-24 Pilot scope review.md")
            .exists());
        assert!(config_vault.join("people/Alex Chen.md").exists());
        assert!(!project_vault
            .join("project-notes/2026-06-24 Pilot scope review.md")
            .exists());
    }

    /// A plan-gated meeting (plan_gated_transcript = true) should still get the
    /// generic placeholder in the vault note — the app-side callout is the only
    /// place for the "drag an export to get transcripts" hint.
    #[test]
    fn plan_gated_meeting_gets_generic_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let meeting = Meeting {
            id: Some("plan-gated-001".to_string()),
            title: "Gated meeting".to_string(),
            created_at: Some("2026-07-01T10:00:00Z".to_string()),
            notes: Some("Some notes here.".to_string()),
            transcript: None,
            people: Vec::new(),
            organizations: Vec::new(),
            plan_gated_transcript: true,
        };
        let result = import_meetings(
            vec![meeting],
            Vec::new(),
            dir.path(),
            &margins_dir,
            &options(),
        )
        .unwrap();
        assert_eq!(result.imported_count, 1);
        let note = std::fs::read_to_string(dir.path().join("meetings/2026-07-01 Gated meeting.md"))
            .unwrap();
        assert!(
            note.contains("_No transcript was present in the export._"),
            "generic placeholder missing for plan-gated meeting:\n{note}"
        );
        assert!(
            !note.contains("_Transcripts aren't available over Granola's API"),
            "CTA text must not appear in vault notes:\n{note}"
        );
    }

    /// A file-import meeting (plan_gated_transcript = false, transcript = None)
    /// must still get the generic placeholder — not the plan-gated CTA.
    #[test]
    fn file_import_meeting_gets_generic_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");
        let meeting = Meeting {
            id: Some("file-import-001".to_string()),
            title: "File import meeting".to_string(),
            created_at: Some("2026-07-02T09:00:00Z".to_string()),
            notes: None,
            transcript: None,
            people: Vec::new(),
            organizations: Vec::new(),
            plan_gated_transcript: false,
        };
        let result = import_meetings(
            vec![meeting],
            Vec::new(),
            dir.path(),
            &margins_dir,
            &options(),
        )
        .unwrap();
        assert_eq!(result.imported_count, 1);
        let note = std::fs::read_to_string(
            dir.path()
                .join("meetings/2026-07-02 File import meeting.md"),
        )
        .unwrap();
        assert!(
            note.contains("_No transcript was present in the export._"),
            "generic placeholder missing:\n{note}"
        );
        assert!(
            !note.contains("_Transcripts aren't available over Granola's API"),
            "plan-gated CTA should not appear for file-import meeting:\n{note}"
        );
    }

    /// A file-drop with a real transcript must replace legacy CTA text that may
    /// already exist in notes written by an older version of the app
    /// (extract_section treats the old CTA string as a placeholder, so a file
    /// import with a real transcript upgrades it cleanly).
    #[test]
    fn file_export_drop_replaces_legacy_cta_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let margins_dir = dir.path().join(".margins");

        // Simulate a note that was written by an older app version containing the
        // CTA text. We write it directly to mimic the legacy state.
        let note_dir = dir.path().join("meetings");
        std::fs::create_dir_all(&note_dir).unwrap();
        let path = note_dir.join("2026-07-03 Replace gated.md");
        let legacy_content = "---\ncreated: '[[2026-07-03]]'\nmargins_session: replace-gated-001\ntitle: 'Replace gated'\nsource: granola\ngranola_id: 'replace-gated-001'\npeople:\norganizations:\n---\n\n# Replace gated\n\n## Granola notes\n\nMCP notes.\n\n## Transcript\n\n_Transcripts aren't available over Granola's API on your plan — drag a Granola export (JSON) onto the sidebar to import them._";
        std::fs::write(&path, legacy_content).unwrap();

        // Now a file export with a real transcript should replace the legacy CTA.
        let file_meeting = Meeting {
            id: Some("replace-gated-001".to_string()),
            title: "Replace gated".to_string(),
            created_at: Some("2026-07-03T08:00:00Z".to_string()),
            notes: Some("File notes.".to_string()),
            transcript: Some("**Me:** real transcript content".to_string()),
            people: Vec::new(),
            organizations: Vec::new(),
            plan_gated_transcript: false,
        };
        import_meetings(
            vec![file_meeting],
            Vec::new(),
            dir.path(),
            &margins_dir,
            &options(),
        )
        .unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(
            updated.contains("**Me:** real transcript content"),
            "real transcript should replace legacy CTA:\n{updated}"
        );
        assert!(
            !updated.contains("_Transcripts aren't available over Granola's API"),
            "legacy CTA should be gone after file export drop:\n{updated}"
        );
    }
}
