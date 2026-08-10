use chrono::{DateTime, Local};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::note_artifacts::read_note_frontmatter;
use margins_store::legacy as session;

/// `(year, month, day, hour, minute, second)` — the second-precision key shared
/// between a session's `start_time` and a note filename's leading timestamp.
type TsKey = (i32, u32, u32, u32, u32, u32);

#[derive(Serialize)]
pub struct SessionInfoDto {
    pub name: String,
    pub project_id: Option<String>,
    pub start_time: String,
    pub notes_path: String,
    pub title: Option<String>,
    pub segment_count: i64,
    pub duration_secs: f64,
    pub memo_line_count: usize,
    pub status: String,
    pub vault_note_path: Option<String>,
    pub failure_message: Option<String>,
    pub processing_state: Option<String>,
    pub failed_stage: Option<String>,
    pub people: Vec<String>,
    pub calendar_event_title: Option<String>,
    pub source: String,
    pub frontmatter_created: Option<String>,
    pub frontmatter_created_sort: Option<String>,
    pub frontmatter_tags: Vec<String>,
    pub frontmatter_people: Vec<String>,
    pub frontmatter_reflection_type: Option<String>,
    pub frontmatter_title: Option<String>,
}

impl SessionInfoDto {
    pub fn display_title(&self) -> String {
        self.frontmatter_title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .or_else(|| {
                self.title
                    .as_deref()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
            })
            .or_else(|| {
                self.calendar_event_title
                    .as_deref()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
            })
            .map(str::to_string)
            .or_else(|| title_from_session_name(&self.name))
            .unwrap_or_else(|| "Untitled capture".to_string())
    }

    pub fn display_people(&self) -> Vec<String> {
        if self.frontmatter_people.is_empty() {
            self.people.clone()
        } else {
            self.frontmatter_people.clone()
        }
    }
}

pub fn list_sessions_with_notes(
    work_dir: &std::path::Path,
    project_id: Option<&str>,
    recording_name: Option<&str>,
) -> Result<Vec<SessionInfoDto>, String> {
    let margins_dir = work_dir.join(".margins");
    // A read must never materialize the vault. On a clean first run the default
    // project points at ~/Documents/margins, but nothing under it should exist
    // until a project is explicitly chosen or capture starts (which creates the
    // dir). Bail before any open_db call — otherwise the tombstone/vault-note
    // reads below would create_dir_all the vault just to list an empty index.
    if !margins_dir.exists() {
        return Ok(Vec::new());
    }
    let tombstoned = session::list_session_tombstone_names(&margins_dir)
        .map(|names| names.into_iter().collect::<HashSet<_>>())
        .unwrap_or_default();
    let sessions: Vec<_> = session::list_sessions(&margins_dir)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|s| !tombstoned.contains(&s.name))
        .collect();
    // Resolve every session's stored note link first (clearing stale rows), so
    // we know which notes are already claimed before attempting any read-time
    // reconciliation. `metas` caches the DB reads for the DTO-building loop.
    let metas: Vec<_> = sessions
        .into_iter()
        .map(|s| {
            let meta = session::get_session_meta(&margins_dir, &s.name);
            (s, meta)
        })
        .collect();

    // Pre-pass: per-second collision counts across all sessions (uniqueness
    // guard) and the set of note paths already claimed by a valid link.
    let mut ts_collisions: HashMap<TsKey, u32> = HashMap::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    let mut any_unprocessed = false;
    for (s, meta) in &metas {
        if let Some(key) = session_ts_key(&s.start_time) {
            *ts_collisions.entry(key).or_insert(0) += 1;
        }
        match meta {
            Ok(m) => match m.vault_note_path.as_ref() {
                Some(path) if PathBuf::from(path).is_file() => {
                    claimed.insert(canonical_path_key(PathBuf::from(path)));
                }
                _ => any_unprocessed = true,
            },
            Err(_) => any_unprocessed = true,
        }
    }
    // Capture notes already procured into the index also count as claimed, so a
    // timestamp/frontmatter match never steals a note another row owns.
    if let Ok(procured) = session::list_vault_notes(&margins_dir) {
        for note in procured {
            claimed.insert(canonical_path_key(PathBuf::from(&note.absolute_path)));
        }
    }

    // Building the note index reads every note's frontmatter, so only do it when
    // there is actually an unprocessed session that could be reconciled.
    let recon_index = if any_unprocessed {
        build_recon_index(work_dir)
    } else {
        ReconIndex::default()
    };

    let mut result = Vec::new();
    let mut represented_note_paths = BTreeSet::new();
    for (s, meta) in metas {
        let (duration, vault_note, status) = match &meta {
            Ok(m) => {
                let dur: f64 = m.segments.iter().filter_map(|seg| seg.duration_secs).sum();
                let mut vnp = m.vault_note_path.as_ref().and_then(|path| {
                    let path_buf = PathBuf::from(path);
                    if path_buf.is_file() {
                        Some(path.clone())
                    } else {
                        let _ = session::clear_vault_note_path(&margins_dir, &s.name);
                        let _ = session::remove_vault_note_by_path(&margins_dir, path);
                        None
                    }
                });
                // Recover a lost link by frontmatter backlink or exact-timestamp
                // filename match. The confident match is persisted after note
                // frontmatter is read below.
                if vnp.is_none() {
                    vnp = reconcile_unprocessed_note(
                        &s.name,
                        &s.start_time,
                        &recon_index,
                        &ts_collisions,
                        &claimed,
                    );
                }
                if let Some(path) = &vnp {
                    represented_note_paths.insert(canonical_path_key(PathBuf::from(path)));
                }
                let failure_message = m.note_error.clone().filter(|msg| !msg.trim().is_empty());
                let st = if vnp.is_some() {
                    "synthesized"
                } else if failure_message.is_some() {
                    "failed"
                } else {
                    "unprocessed"
                };
                (dur, vnp, st.to_string())
            }
            Err(_) => (0.0, None, "unprocessed".to_string()),
        };

        let notes_file = work_dir.join(&s.notes_path);
        let memo_count = std::fs::read_to_string(&notes_file)
            .map(|c| c.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0);

        let is_recording = recording_name == Some(s.name.as_str());
        let frontmatter = read_note_frontmatter(vault_note.as_deref());
        let people = if frontmatter.people_present {
            frontmatter.people.clone()
        } else {
            meta.as_ref().map(|m| m.people.clone()).unwrap_or_default()
        };
        let frontmatter_title = derive_note_frontmatter_title(vault_note.as_deref());
        let display_note_title = vault_note
            .as_deref()
            .and_then(|path| derive_note_display_title(std::path::Path::new(path)));
        if let Some(path) = vault_note.as_deref() {
            let should_sync = meta.as_ref().ok().is_some_and(|m| {
                m.vault_note_path.as_deref() != Some(path)
                    || frontmatter_title
                        .as_deref()
                        .is_some_and(|title| m.title.as_deref() != Some(title))
                    || (frontmatter.people_present && m.people != frontmatter.people)
            });
            if should_sync {
                let _ = session::sync_session_note_metadata(
                    &margins_dir,
                    &s.name,
                    path,
                    frontmatter_title.as_deref(),
                    &frontmatter.people,
                    frontmatter.people_present,
                );
            }
        }
        let display_status = if is_recording {
            "recording".into()
        } else {
            status
        };
        let failure_message = if display_status == "failed" {
            meta.as_ref().ok().and_then(|m| m.note_error.clone())
        } else {
            None
        };
        let processing_state = meta.as_ref().ok().and_then(|m| m.processing_state.clone());
        let failed_stage = meta.as_ref().ok().and_then(|m| m.failed_stage.clone());

        result.push(SessionInfoDto {
            name: s.name.clone(),
            project_id: project_id.map(str::to_string),
            start_time: s.start_time.clone(),
            notes_path: s.notes_path.clone(),
            title: display_note_title
                .clone()
                .or_else(|| meta.as_ref().ok().and_then(|m| m.title.clone())),
            segment_count: s.segment_count,
            duration_secs: duration,
            memo_line_count: memo_count,
            status: display_status,
            vault_note_path: vault_note,
            failure_message,
            processing_state,
            failed_stage,
            people,
            calendar_event_title: meta
                .as_ref()
                .ok()
                .and_then(|m| m.calendar_event.as_ref().map(|e| e.title.clone())),
            source: "session".to_string(),
            frontmatter_created: frontmatter.created,
            frontmatter_created_sort: frontmatter.created_sort,
            frontmatter_tags: frontmatter.tags,
            frontmatter_people: frontmatter.people,
            frontmatter_reflection_type: frontmatter.reflection_type,
            frontmatter_title: display_note_title,
        });
    }

    result.extend(list_procured_vault_notes(
        &margins_dir,
        &represented_note_paths,
        &tombstoned,
        project_id,
    ));
    result.sort_by(|a, b| {
        let a_key = a
            .frontmatter_created_sort
            .as_deref()
            .unwrap_or(&a.start_time);
        let b_key = b
            .frontmatter_created_sort
            .as_deref()
            .unwrap_or(&b.start_time);
        b_key.cmp(a_key).then_with(|| a.name.cmp(&b.name))
    });
    Ok(result)
}

pub fn recent_people_candidates(
    work_dir: &std::path::Path,
    people_folder: &str,
    project_id: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(sessions) = list_sessions_with_notes(work_dir, project_id, None) {
        for session in sessions {
            for person in session
                .frontmatter_people
                .iter()
                .chain(session.people.iter())
            {
                push_people_candidate(&mut out, person);
                if out.len() >= 50 {
                    return out;
                }
            }
        }
    }
    let people_dir = work_dir.join(people_folder.trim());
    if let Ok(entries) = std::fs::read_dir(people_dir) {
        let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
        entries.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .map(std::cmp::Reverse)
        });
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                push_people_candidate(&mut out, stem);
                if out.len() >= 50 {
                    return out;
                }
            }
        }
    }
    out
}

fn push_people_candidate(out: &mut Vec<String>, raw: &str) {
    let person = raw
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim()
        .strip_prefix("[[")
        .and_then(|v| v.strip_suffix("]]"))
        .unwrap_or(raw.trim())
        .split('|')
        .next()
        .unwrap_or("")
        .trim();
    if person.is_empty() {
        return;
    }
    if !out
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(person))
    {
        out.push(person.to_string());
    }
}

fn list_procured_vault_notes(
    margins_dir: &std::path::Path,
    represented_note_paths: &BTreeSet<String>,
    tombstoned: &HashSet<String>,
    project_id: Option<&str>,
) -> Vec<SessionInfoDto> {
    let Ok(procured_notes) = session::list_vault_notes(margins_dir) else {
        return Vec::new();
    };
    let mut notes = Vec::new();

    for note in procured_notes {
        if tombstoned.contains(&note.id) {
            continue;
        }
        if note
            .source_session_name
            .as_deref()
            .is_some_and(|name| tombstoned.contains(name))
        {
            continue;
        }
        let path = PathBuf::from(&note.absolute_path);
        if !path.is_file() {
            let _ = session::remove_vault_note_by_id(margins_dir, &note.id);
            continue;
        }
        if is_internal_margins_path(margins_dir, &path) {
            continue;
        }
        let path_key = canonical_path_key(path.clone());
        if represented_note_paths.contains(&path_key) {
            continue;
        }

        let modified = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Local>::from)
            .unwrap_or_else(Local::now);
        let title = derive_note_display_title(&path).unwrap_or_else(|| "Untitled note".to_string());
        let frontmatter = read_note_frontmatter(path.to_str());

        notes.push(SessionInfoDto {
            name: note.id,
            project_id: project_id.map(str::to_string),
            start_time: modified.to_rfc3339(),
            notes_path: path.to_string_lossy().to_string(),
            title: Some(title),
            segment_count: 0,
            duration_secs: 0.0,
            memo_line_count: 0,
            status: "synthesized".to_string(),
            vault_note_path: Some(path_key),
            failure_message: None,
            processing_state: Some("done".to_string()),
            failed_stage: None,
            people: Vec::new(),
            calendar_event_title: None,
            source: "capture_note".to_string(),
            frontmatter_created: frontmatter.created,
            frontmatter_created_sort: frontmatter.created_sort,
            frontmatter_tags: frontmatter.tags,
            frontmatter_people: frontmatter.people,
            frontmatter_reflection_type: frontmatter.reflection_type,
            frontmatter_title: derive_note_display_title(&path),
        });
    }

    notes.sort_by(|a, b| {
        let a_key = a
            .frontmatter_created_sort
            .as_deref()
            .unwrap_or(&a.start_time);
        let b_key = b
            .frontmatter_created_sort
            .as_deref()
            .unwrap_or(&b.start_time);
        b_key.cmp(a_key).then_with(|| a.name.cmp(&b.name))
    });
    notes.truncate(100);
    notes
}

fn canonical_path_key(path: PathBuf) -> String {
    path.canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn is_internal_margins_path(margins_dir: &Path, path: &Path) -> bool {
    let margins_dir = margins_dir
        .canonicalize()
        .unwrap_or_else(|_| margins_dir.to_path_buf());
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(margins_dir)
}

/// Indexes of the notes living in `work_dir`, used for read-time reconciliation
/// of sessions whose stored note link was lost (renamed file / DB reset).
#[derive(Default)]
struct ReconIndex {
    /// Filename leading-timestamp prefix → note paths bearing it.
    ts_index: HashMap<TsKey, Vec<PathBuf>>,
    /// `margins_session:` frontmatter value → note paths bearing it.
    session_index: HashMap<String, Vec<PathBuf>>,
}

/// Scan `work_dir`, indexing each `*.md` note by (a) the leading timestamp in
/// its filename and (b) any `margins_session:` frontmatter backlink. The scan is
/// recursive because Obsidian notes commonly move between folders after save.
/// Reads each note's frontmatter, so callers should only build this when a
/// reconciliation candidate (an unprocessed session) actually exists.
fn build_recon_index(work_dir: &Path) -> ReconIndex {
    let mut index = ReconIndex::default();
    scan_recon_dir(work_dir, &mut index, 0);
    index
}

fn scan_recon_dir(dir: &Path, index: &mut ReconIndex, depth: usize) {
    if depth > 8 || should_skip_recon_dir(dir, depth) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_recon_dir(&path, index, depth + 1);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") || !path.is_file() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(key) = parse_leading_note_timestamp(name) {
                index.ts_index.entry(key).or_default().push(path.clone());
            }
        }
        if let Some(session) = read_note_frontmatter(path.to_str()).margins_session {
            let session = session.trim().to_string();
            if !session.is_empty() {
                index
                    .session_index
                    .entry(session)
                    .or_default()
                    .push(path.clone());
            }
        }
    }
}

fn should_skip_recon_dir(path: &Path, depth: usize) -> bool {
    if depth == 0 {
        return false;
    }
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | ".margins" | ".obsidian" | ".trash" | "node_modules" | "target")
    )
}

/// Try to recover a confident note link for an unprocessed session. Prefers the
/// durable `margins_session:` frontmatter backlink, then falls back to an
/// exact-timestamp filename match guarded against known false positives
/// (placeholder-timestamp collisions, already-claimed notes, ambiguous
/// prefixes). Returns the matched note path, or None.
fn reconcile_unprocessed_note(
    session_name: &str,
    start_time: &str,
    index: &ReconIndex,
    ts_collisions: &HashMap<TsKey, u32>,
    claimed: &BTreeSet<String>,
) -> Option<String> {
    // 1) Frontmatter backlink: robust across renames and reformats. Confident
    //    when exactly one note bears this session id and it isn't already owned.
    if let Some(paths) = index.session_index.get(session_name) {
        if paths.len() == 1 && !claimed.contains(&canonical_path_key(paths[0].clone())) {
            return Some(paths[0].to_string_lossy().to_string());
        }
    }

    // 2) Exact start-time → filename-timestamp prefix, with the report's guards.
    let key = session_ts_key(start_time)?;
    // Guard: this exact second must be unique across all sessions (excludes the
    // union-square-park placeholder-timestamp collisions).
    if ts_collisions.get(&key).copied().unwrap_or(0) != 1 {
        return None;
    }
    // Guard: exactly one note carries that second-precision prefix.
    let paths = index.ts_index.get(&key)?;
    if paths.len() != 1 {
        return None;
    }
    // Guard: that note isn't already claimed by another session/capture note.
    if claimed.contains(&canonical_path_key(paths[0].clone())) {
        return None;
    }
    Some(paths[0].to_string_lossy().to_string())
}

/// Leading `YYYY-MM-DD-H-M-S` of a note filename (hour 1–2 digits, minute/second
/// 2 digits). Template-independent on the timestamp portion. Returns None when
/// the filename doesn't open with a full second-precision timestamp.
fn parse_leading_note_timestamp(name: &str) -> Option<TsKey> {
    let mut it = name.splitn(7, '-');
    let y = it.next()?.parse().ok()?;
    let mo = it.next()?.parse().ok()?;
    let d = it.next()?.parse().ok()?;
    let h = it.next()?.parse().ok()?;
    let mi = it.next()?.parse().ok()?;
    // The final field starts the second + title run; take the 2-digit second head.
    let rest = it.next()?;
    let s: u32 = rest.get(..2)?.parse().ok()?;
    Some((y, mo, d, h, mi, s))
}

/// Second-precision key for a session's RFC3339 `start_time`, in its stored
/// local offset (matching how the note filename timestamp was formatted).
fn session_ts_key(start_time: &str) -> Option<TsKey> {
    use chrono::{DateTime, Datelike, Timelike};
    let dt = DateTime::parse_from_rfc3339(start_time).ok()?;
    Some((
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
    ))
}

/// Derive the best metadata title for a connected note: prefer the parsed
/// frontmatter `title:` scalar, else fall back to the first `# ` H1 heading.
/// Returns None when neither exists.
fn derive_note_frontmatter_title(path: Option<&str>) -> Option<String> {
    let path = path?;
    if let Some(title) = read_note_frontmatter(Some(path)).title {
        return Some(title);
    }
    capture_note_title(std::path::Path::new(path))
}

fn derive_note_display_title(path: &std::path::Path) -> Option<String> {
    derive_note_frontmatter_title(path.to_str()).or_else(|| note_title_from_filename(path))
}

fn capture_note_title(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines().take(80) {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

fn humanize_note_stem(stem: &str) -> String {
    let text = stem.replace(['-', '_'], " ");
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        "Untitled note".to_string()
    } else {
        compact
    }
}

fn note_title_from_filename(path: &std::path::Path) -> Option<String> {
    let stem = path.file_stem().and_then(|s| s.to_str())?;
    Some(humanize_note_stem(strip_leading_timestamp_stem(stem)))
        .filter(|title| title != "Untitled note")
}

fn strip_leading_timestamp_stem(stem: &str) -> &str {
    if parse_leading_note_timestamp(stem).is_none() {
        return stem;
    }
    let mut dash_count = 0;
    for (idx, ch) in stem.char_indices() {
        if ch != '-' {
            continue;
        }
        dash_count += 1;
        if dash_count != 5 {
            continue;
        }
        let rest = &stem[idx + 1..];
        let second_len = rest
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .take(2)
            .map(char::len_utf8)
            .sum::<usize>();
        if second_len == 2 {
            return rest[second_len..].trim_start_matches([' ', '-', '_']);
        }
        break;
    }
    stem
}

fn title_from_session_name(name: &str) -> Option<String> {
    if is_timestamp_session_name(name) {
        return None;
    }
    let title = name
        .replace(['-', '_'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

fn is_timestamp_session_name(name: &str) -> bool {
    let parts = name.split('-').collect::<Vec<_>>();
    if !(5..=7).contains(&parts.len()) {
        return false;
    }
    parts[0].len() == 4
        && parts[1].len() == 2
        && parts[2].len() == 2
        && parts[3].len() == 2
        && parts[4].len() == 2
        && parts
            .iter()
            .enumerate()
            .all(|(idx, part)| idx == 6 || part.chars().all(|ch| ch.is_ascii_digit()))
}

pub fn note_path_for_session_or_capture(
    margins_dir: &std::path::Path,
    name: &str,
) -> Result<PathBuf, String> {
    if let Ok(meta) = session::get_session_meta(margins_dir, name) {
        if let Some(path) = meta.vault_note_path.as_ref() {
            let path_buf = PathBuf::from(path);
            if path_buf.is_file() {
                return Ok(path_buf);
            }
            let _ = session::clear_vault_note_path(margins_dir, name);
            let _ = session::remove_vault_note_by_path(margins_dir, path);
        }
        if let Some(work_dir) = margins_dir.parent() {
            let index = build_recon_index(work_dir);
            let ts_collisions = session_ts_collisions(margins_dir);
            let claimed = claimed_note_paths(margins_dir);
            if let Some(path) =
                reconcile_unprocessed_note(name, &meta.start_time, &index, &ts_collisions, &claimed)
            {
                return Ok(PathBuf::from(path));
            }
        }
    }
    if let Some(path) =
        session::vault_note_path_by_id(margins_dir, name).map_err(|e| e.to_string())?
    {
        let path_buf = PathBuf::from(&path);
        if path_buf.is_file() {
            return Ok(path_buf);
        }
        let _ = session::remove_vault_note_by_id(margins_dir, name);
    }
    Err("No saved note for this session yet.".to_string())
}

fn session_ts_collisions(margins_dir: &Path) -> HashMap<TsKey, u32> {
    let mut out = HashMap::new();
    if let Ok(sessions) = session::list_sessions(margins_dir) {
        for session in sessions {
            if let Some(key) = session_ts_key(&session.start_time) {
                *out.entry(key).or_insert(0) += 1;
            }
        }
    }
    out
}

/// Names of sessions whose stored vault note file was *genuinely* deleted — the
/// link cannot be recovered by the durable `margins_session:` frontmatter
/// backlink or a unique exact-timestamp filename match. These are the sessions
/// the sidebar refresh should purge: the user removed the note, so the recording
/// and its DB rows are cruft.
///
/// Excludes:
/// - Sessions that never had a note (never distilled) — legit pending work.
/// - Sessions whose note was merely renamed/moved — reconciliation recovers those.
pub fn sessions_with_deleted_notes(work_dir: &std::path::Path) -> Vec<String> {
    let margins_dir = work_dir.join(".margins");
    if !margins_dir.exists() {
        return Vec::new();
    }
    let sessions = match session::list_sessions(&margins_dir) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    // Per-second collision counts across all sessions (the timestamp-match guard),
    // plus the candidates whose stored note path no longer points at a file.
    let mut ts_collisions: HashMap<TsKey, u32> = HashMap::new();
    let mut candidates: Vec<(String, String)> = Vec::new();
    for s in &sessions {
        if let Some(key) = session_ts_key(&s.start_time) {
            *ts_collisions.entry(key).or_insert(0) += 1;
        }
        if let Ok(meta) = session::get_session_meta(&margins_dir, &s.name) {
            let missing_linked_note = meta
                .vault_note_path
                .as_ref()
                .is_some_and(|path| !PathBuf::from(path).is_file());
            // Ordinary session listing clears stale note paths. Preserve the
            // ability to prune afterward by treating a completed session with
            // no remaining link as a deletion candidate. Never-distilled
            // sessions remain in processing_state `none` and are kept.
            let completed_without_note =
                meta.vault_note_path.is_none() && meta.processing_state.as_deref() == Some("done");
            if missing_linked_note || completed_without_note {
                candidates.push((s.name.clone(), s.start_time.clone()));
            }
        }
    }
    if candidates.is_empty() {
        return Vec::new();
    }
    // A candidate is only a genuine deletion when read-time reconciliation fails
    // to find a replacement note anywhere in the vault.
    let index = build_recon_index(work_dir);
    let claimed = claimed_note_paths(&margins_dir);
    candidates
        .into_iter()
        .filter(|(name, start_time)| {
            reconcile_unprocessed_note(name, start_time, &index, &ts_collisions, &claimed).is_none()
        })
        .map(|(name, _)| name)
        .collect()
}

fn claimed_note_paths(margins_dir: &Path) -> BTreeSet<String> {
    let mut claimed = BTreeSet::new();
    if let Ok(sessions) = session::list_sessions(margins_dir) {
        for session in sessions {
            if session.name == "" {
                continue;
            }
            if let Ok(meta) = session::get_session_meta(margins_dir, &session.name) {
                if let Some(path) = meta.vault_note_path.as_ref() {
                    let path_buf = PathBuf::from(path);
                    if path_buf.is_file() {
                        claimed.insert(canonical_path_key(path_buf));
                    }
                }
            }
        }
    }
    if let Ok(procured) = session::list_vault_notes(margins_dir) {
        for note in procured {
            claimed.insert(canonical_path_key(PathBuf::from(&note.absolute_path)));
        }
    }
    claimed
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Local, TimeZone};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A fixed local timestamp plus the matching note-filename prefix it produces
    /// under the default `%Y-%m-%d-%-H-%M-%S` template.
    fn fixed_dt(h: u32, mi: u32, s: u32) -> DateTime<Local> {
        match Local.with_ymd_and_hms(2026, 5, 18, h, mi, s) {
            chrono::LocalResult::Single(dt) => dt,
            chrono::LocalResult::Ambiguous(dt, _) => dt,
            chrono::LocalResult::None => panic!("invalid fixed datetime"),
        }
    }

    fn note_prefix(dt: &DateTime<Local>) -> String {
        dt.format("%Y-%m-%d-%-H-%M-%S").to_string()
    }

    fn temp_work_dir(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("margins-session-index-{label}-{unique}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn empty_work_dir_lists_no_sessions_or_placeholder_notes() {
        let work_dir = temp_work_dir("empty");

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert!(sessions.is_empty());
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn stale_session_vault_note_is_cleared_and_listed_as_unprocessed() {
        let work_dir = temp_work_dir("stale-session-note");
        let margins_dir = work_dir.join(".margins");
        session::create_session(&margins_dir, "meet", &Local::now(), "meet.md").unwrap();
        let missing_note = work_dir.join("vault").join("missing.md");
        session::set_vault_note_path(&margins_dir, "meet", &missing_note.to_string_lossy())
            .unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "meet");
        assert_eq!(sessions[0].status, "unprocessed");
        assert_eq!(sessions[0].vault_note_path, None);
        let meta = session::get_session_meta(&margins_dir, "meet").unwrap();
        assert_eq!(meta.vault_note_path, None);
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn missing_capture_note_index_row_is_filtered_and_repaired() {
        let work_dir = temp_work_dir("missing-capture-note");
        let margins_dir = work_dir.join(".margins");
        let missing_note = work_dir.join("vault").join("missing.md");
        session::set_vault_note_path(&margins_dir, "ghost", &missing_note.to_string_lossy())
            .unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert!(sessions.is_empty());
        assert!(session::list_vault_notes(&margins_dir).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn durable_note_error_lists_session_as_failed() {
        let work_dir = temp_work_dir("failed-session");
        let margins_dir = work_dir.join(".margins");
        session::create_session(&margins_dir, "meet", &Local::now(), "meet.md").unwrap();
        session::set_note_error(&margins_dir, "meet", "AI note distillation failed").unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "meet");
        assert_eq!(sessions[0].status, "failed");
        assert_eq!(
            sessions[0].failure_message.as_deref(),
            Some("AI note distillation failed")
        );
        assert_eq!(sessions[0].vault_note_path, None);
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn existing_capture_note_index_row_resolves_to_note_path() {
        let work_dir = temp_work_dir("capture-note");
        let margins_dir = work_dir.join(".margins");
        let vault = work_dir.join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let note_path = vault.join("daily.md");
        std::fs::write(&note_path, "# Daily\n\nBody").unwrap();
        session::set_vault_note_path(&margins_dir, "ghost", &note_path.to_string_lossy()).unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].source, "capture_note");
        assert_eq!(sessions[0].title.as_deref(), Some("Daily"));
        let resolved = note_path_for_session_or_capture(&margins_dir, &sessions[0].name).unwrap();
        assert_eq!(resolved, note_path);
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn tombstoned_capture_note_index_row_is_not_listed() {
        let work_dir = temp_work_dir("deleted-capture-note");
        let margins_dir = work_dir.join(".margins");
        let vault = work_dir.join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let note_path = vault.join("deleted.md");
        std::fs::write(&note_path, "# Deleted\n\nBody").unwrap();
        session::set_vault_note_path(&margins_dir, "ghost", &note_path.to_string_lossy()).unwrap();
        session::begin_delete_session(&margins_dir, "ghost").unwrap();
        session::finalize_delete_session(&margins_dir, "ghost").unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert!(sessions.is_empty());
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn tombstoned_procured_note_id_is_not_listed() {
        let work_dir = temp_work_dir("deleted-capture-note-id");
        let margins_dir = work_dir.join(".margins");
        let vault = work_dir.join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let note_path = vault.join("deleted-by-id.md");
        std::fs::write(&note_path, "# Deleted by id\n\nBody").unwrap();
        session::set_vault_note_path(&margins_dir, "ghost", &note_path.to_string_lossy()).unwrap();
        let note_id = session::list_vault_notes(&margins_dir).unwrap()[0]
            .id
            .clone();
        session::begin_delete_session(&margins_dir, &note_id).unwrap();
        session::finalize_delete_session(&margins_dir, &note_id).unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert!(sessions.is_empty());
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn capture_note_title_falls_back_to_current_filename() {
        let work_dir = temp_work_dir("capture-note-filename-title");
        let margins_dir = work_dir.join(".margins");
        let vault = work_dir.join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let note_path = vault.join("2026-05-18-13-31-09 wedge before memory.md");
        std::fs::write(&note_path, "Body without title metadata").unwrap();
        session::set_vault_note_path(&margins_dir, "ghost", &note_path.to_string_lossy()).unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("wedge before memory"));
        assert_eq!(
            sessions[0].frontmatter_title.as_deref(),
            Some("wedge before memory")
        );
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn imported_session_with_db_row_and_vault_note_surfaces_as_session_not_capture_note() {
        let work_dir = temp_work_dir("imported-session");
        let margins_dir = work_dir.join(".margins");
        let vault = work_dir.join("vault");
        std::fs::create_dir_all(&vault).unwrap();

        // Create a real session via the DB (simulating import_audio_file's flow).
        session::create_session(
            &margins_dir,
            "imported_audio",
            &Local::now(),
            "imported_audio.md",
        )
        .unwrap();
        // Add a segment (simulating the imported audio downmix).
        session::add_segment(
            &margins_dir,
            "imported_audio",
            0,
            ".margins/imported_audio_seg0.wav",
            0,
            Some(30.0),
        )
        .unwrap();

        // Register a vault note for this session (simulating process_session's distillation).
        let note_path = vault.join("imported_2024.md");
        std::fs::write(&note_path, "# Imported Audio\n\nDistilled notes").unwrap();
        session::set_vault_note_path(&margins_dir, "imported_audio", &note_path.to_string_lossy())
            .unwrap();

        // List sessions: the imported session should appear exactly once with source=="session".
        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        let imported = sessions
            .iter()
            .find(|s| s.name == "imported_audio")
            .expect("imported session should be in list");

        assert_eq!(
            imported.source, "session",
            "imported session must have source==\"session\", not deduplicated to capture_note"
        );
        assert_eq!(
            imported.status, "synthesized",
            "imported session with vault_note should be synthesized"
        );
        assert_eq!(
            imported.vault_note_path.as_deref(),
            Some(note_path.to_string_lossy().as_ref())
        );

        // Verify no duplicate capture_note appears for the same vault note path.
        let capture_notes: Vec<_> = sessions
            .iter()
            .filter(|s| s.source == "capture_note")
            .collect();
        for cn in &capture_notes {
            assert_ne!(
                cn.vault_note_path, imported.vault_note_path,
                "vault note should not appear as both session and capture_note"
            );
        }

        let _ = std::fs::remove_dir_all(work_dir);
    }

    // --- Read-time reconciliation -----------------------------------------

    fn find<'a>(sessions: &'a [SessionInfoDto], name: &str) -> &'a SessionInfoDto {
        sessions
            .iter()
            .find(|s| s.name == name)
            .expect("session should be listed")
    }

    #[test]
    fn unique_timestamp_with_single_note_relinks_as_synthesized() {
        let work_dir = temp_work_dir("recon-unique-ts");
        let margins_dir = work_dir.join(".margins");
        let dt = fixed_dt(13, 31, 9);
        session::create_session(&margins_dir, "neil3", &dt, "neil3.md").unwrap();
        let note = work_dir.join(format!("{} chat with neil.md", note_prefix(&dt)));
        std::fs::write(&note, "# Neil\n\nBody").unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        let neil = find(&sessions, "neil3");
        assert_eq!(neil.status, "synthesized");
        assert_eq!(neil.vault_note_path.as_deref(), note.to_str());
        // The matched note must NOT also surface as a standalone capture_note.
        assert!(sessions.iter().all(|s| s.source != "capture_note"));
        // Durable: the recovered link is written back so the sidebar and note
        // commands stop carrying a stale/unprocessed handle.
        let meta = session::get_session_meta(&margins_dir, "neil3").unwrap();
        assert_eq!(meta.vault_note_path.as_deref(), note.to_str());
        assert_eq!(meta.title.as_deref(), Some("Neil"));
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn shared_timestamp_across_sessions_is_not_linked() {
        let work_dir = temp_work_dir("recon-shared-ts");
        let margins_dir = work_dir.join(".margins");
        let dt = fixed_dt(19, 13, 0);
        session::create_session(&margins_dir, "usp-a", &dt, "usp-a.md").unwrap();
        session::create_session(&margins_dir, "usp-b", &dt, "usp-b.md").unwrap();
        let note = work_dir.join(format!("{} union square park.md", note_prefix(&dt)));
        std::fs::write(&note, "# USP\n\nBody").unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert_eq!(find(&sessions, "usp-a").status, "unprocessed");
        assert_eq!(find(&sessions, "usp-b").status, "unprocessed");
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn note_already_claimed_by_another_session_is_not_relinked() {
        let work_dir = temp_work_dir("recon-claimed");
        let margins_dir = work_dir.join(".margins");
        // The note's filename timestamp matches `other`, but `owner` already
        // holds a valid link to it — the claimed guard must block the steal.
        let dt = fixed_dt(8, 5, 30);
        let note = work_dir.join(format!("{} standup.md", note_prefix(&dt)));
        std::fs::write(&note, "# Standup\n\nBody").unwrap();

        session::create_session(&margins_dir, "owner", &fixed_dt(7, 0, 0), "owner.md").unwrap();
        session::set_vault_note_path(&margins_dir, "owner", &note.to_string_lossy()).unwrap();
        session::create_session(&margins_dir, "other", &dt, "other.md").unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert_eq!(find(&sessions, "other").status, "unprocessed");
        assert_eq!(find(&sessions, "other").vault_note_path, None);
        // Owner keeps its legitimate link.
        assert_eq!(find(&sessions, "owner").status, "synthesized");
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn two_notes_with_same_timestamp_are_not_linked() {
        let work_dir = temp_work_dir("recon-ambiguous-notes");
        let margins_dir = work_dir.join(".margins");
        let dt = fixed_dt(10, 0, 0);
        session::create_session(&margins_dir, "amb", &dt, "amb.md").unwrap();
        std::fs::write(
            work_dir.join(format!("{} first.md", note_prefix(&dt))),
            "# A",
        )
        .unwrap();
        std::fs::write(
            work_dir.join(format!("{} second.md", note_prefix(&dt))),
            "# B",
        )
        .unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert_eq!(find(&sessions, "amb").status, "unprocessed");
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn session_without_matching_note_stays_unprocessed() {
        let work_dir = temp_work_dir("recon-no-note");
        let margins_dir = work_dir.join(".margins");
        session::create_session(&margins_dir, "lonely", &fixed_dt(9, 9, 9), "lonely.md").unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        assert_eq!(find(&sessions, "lonely").status, "unprocessed");
        assert_eq!(find(&sessions, "lonely").vault_note_path, None);
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn frontmatter_backlink_relinks_even_without_timestamp_match() {
        let work_dir = temp_work_dir("recon-frontmatter");
        let margins_dir = work_dir.join(".margins");
        // Session start time has no matching filename prefix; the note is
        // recovered purely via the durable `margins_session:` backlink.
        session::create_session(&margins_dir, "willie", &fixed_dt(12, 5, 38), "willie.md").unwrap();
        let note = work_dir.join("a-renamed-note-title.md");
        std::fs::write(
            &note,
            "---\nmargins_session: 'willie'\ncreated: '[[2026-05-29]]'\n---\n# Willie\n",
        )
        .unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        let willie = find(&sessions, "willie");
        assert_eq!(willie.status, "synthesized");
        assert_eq!(willie.vault_note_path.as_deref(), note.to_str());
        assert_eq!(willie.title.as_deref(), Some("Willie"));
        assert!(sessions.iter().all(|s| s.source != "capture_note"));
        let meta = session::get_session_meta(&margins_dir, "willie").unwrap();
        assert_eq!(meta.vault_note_path.as_deref(), note.to_str());
        assert_eq!(meta.title.as_deref(), Some("Willie"));
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn frontmatter_backlink_relinks_nested_note_and_syncs_people() {
        let work_dir = temp_work_dir("recon-nested-frontmatter");
        let margins_dir = work_dir.join(".margins");
        let inbox = work_dir.join("obsidian").join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        session::create_session(
            &margins_dir,
            "nested-call",
            &fixed_dt(14, 0, 0),
            "nested-call.md",
        )
        .unwrap();
        let note = inbox.join("renamed call.md");
        std::fs::write(
            &note,
            "---\ntitle: 'Renamed call'\nmargins_session: nested-call\npeople:\n  - '[[Ada Lovelace]]'\n---\n# Old heading\n",
        )
        .unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        let session = find(&sessions, "nested-call");
        assert_eq!(session.status, "synthesized");
        assert_eq!(session.vault_note_path.as_deref(), note.to_str());
        assert_eq!(session.frontmatter_title.as_deref(), Some("Renamed call"));
        assert_eq!(session.frontmatter_people, vec!["Ada Lovelace"]);
        let meta = session::get_session_meta(&margins_dir, "nested-call").unwrap();
        assert_eq!(meta.vault_note_path.as_deref(), note.to_str());
        assert_eq!(meta.title.as_deref(), Some("Renamed call"));
        assert_eq!(meta.people, vec!["Ada Lovelace"]);
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn explicit_empty_people_frontmatter_clears_display_people() {
        let work_dir = temp_work_dir("recon-empty-people");
        let margins_dir = work_dir.join(".margins");
        let inbox = work_dir.join("notes");
        std::fs::create_dir_all(&inbox).unwrap();
        session::create_session(&margins_dir, "empty-people", &fixed_dt(14, 0, 0), "call.md")
            .unwrap();
        session::set_people(
            &margins_dir,
            "empty-people",
            vec!["Ada Lovelace".to_string()],
        )
        .unwrap();
        let note = inbox.join("call.md");
        session::set_vault_note_path(&margins_dir, "empty-people", &note.to_string_lossy())
            .unwrap();
        std::fs::write(&note, "---\ntitle: 'Call'\npeople:\n---\n# Call\n").unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        let session = find(&sessions, "empty-people");
        assert!(session.people.is_empty());
        assert!(session.frontmatter_people.is_empty());
        assert!(session.display_people().is_empty());
        let meta = session::get_session_meta(&margins_dir, "empty-people").unwrap();
        assert!(meta.people.is_empty());
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn frontmatter_backlink_repairs_stale_session_path_after_external_rename() {
        let work_dir = temp_work_dir("recon-stale-renamed");
        let margins_dir = work_dir.join(".margins");
        let vault = work_dir.join("obsidian").join("notes");
        std::fs::create_dir_all(&vault).unwrap();
        session::create_session(&margins_dir, "willie", &fixed_dt(12, 5, 38), "willie.md").unwrap();
        let old_note = vault.join("untitled.md");
        let renamed_note = vault.join("wedge before memory.md");
        session::set_vault_note_path(&margins_dir, "willie", &old_note.to_string_lossy()).unwrap();
        std::fs::write(
            &renamed_note,
            "---\nmargins_session: 'willie'\ncreated: '[[2026-05-29]]'\n---\nBody\n",
        )
        .unwrap();

        let sessions = list_sessions_with_notes(&work_dir, None, None).unwrap();

        let willie = find(&sessions, "willie");
        assert_eq!(willie.status, "synthesized");
        assert_eq!(willie.vault_note_path.as_deref(), renamed_note.to_str());
        assert_eq!(
            willie.frontmatter_title.as_deref(),
            Some("wedge before memory")
        );
        let meta = session::get_session_meta(&margins_dir, "willie").unwrap();
        assert_eq!(meta.vault_note_path.as_deref(), renamed_note.to_str());
        assert!(sessions.iter().all(|s| s.source != "capture_note"));
        let _ = std::fs::remove_dir_all(work_dir);
    }

    // --- Deleted-note detection (sidebar refresh prune) -------------------

    #[test]
    fn deleted_note_flags_session_for_prune() {
        let work_dir = temp_work_dir("prune-deleted-note");
        let margins_dir = work_dir.join(".margins");
        session::create_session(&margins_dir, "gone", &fixed_dt(9, 0, 0), "gone.md").unwrap();
        // Stored link points at a note that no longer exists on disk.
        let missing = work_dir.join("inbox").join("gone.md");
        session::set_vault_note_path(&margins_dir, "gone", &missing.to_string_lossy()).unwrap();

        let deleted = sessions_with_deleted_notes(&work_dir);

        assert_eq!(deleted, vec!["gone".to_string()]);
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn never_distilled_session_is_not_flagged_for_prune() {
        let work_dir = temp_work_dir("prune-never-distilled");
        let margins_dir = work_dir.join(".margins");
        // No vault_note_path ever set — legit pending work, must be kept.
        session::create_session(&margins_dir, "pending", &fixed_dt(9, 0, 0), "pending.md").unwrap();

        let deleted = sessions_with_deleted_notes(&work_dir);

        assert!(deleted.is_empty());
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn completed_session_is_flagged_after_listing_clears_deleted_note_path() {
        let work_dir = temp_work_dir("prune-after-list");
        let margins_dir = work_dir.join(".margins");
        session::create_session(&margins_dir, "gone", &fixed_dt(9, 0, 0), "gone.md").unwrap();
        let missing = work_dir.join("inbox").join("gone.md");
        session::set_vault_note_path(&margins_dir, "gone", &missing.to_string_lossy()).unwrap();
        session::set_processing_state(&margins_dir, "gone", "done", None).unwrap();

        // Rendering the sidebar first clears the stale vault_note_path.
        let _ = list_sessions_with_notes(&work_dir, None, None).unwrap();
        assert_eq!(
            session::get_session_meta(&margins_dir, "gone")
                .unwrap()
                .vault_note_path,
            None
        );

        let deleted = sessions_with_deleted_notes(&work_dir);

        assert_eq!(deleted, vec!["gone".to_string()]);
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn renamed_note_is_not_flagged_for_prune() {
        let work_dir = temp_work_dir("prune-renamed-note");
        let margins_dir = work_dir.join(".margins");
        session::create_session(&margins_dir, "willie", &fixed_dt(12, 5, 38), "willie.md").unwrap();
        // Stored path is stale, but a note with the durable backlink still exists.
        let stale = work_dir.join("inbox").join("untitled.md");
        session::set_vault_note_path(&margins_dir, "willie", &stale.to_string_lossy()).unwrap();
        let renamed = work_dir.join("renamed-note.md");
        std::fs::write(&renamed, "---\nmargins_session: 'willie'\n---\n# Willie\n").unwrap();

        let deleted = sessions_with_deleted_notes(&work_dir);

        assert!(deleted.is_empty());
        let _ = std::fs::remove_dir_all(work_dir);
    }
}
