use crate::integrations::Importer;
use anyhow::{bail, Context, Result as AnyResult};
use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GranolaImportSurvey {
    pub file_count: usize,
    pub meeting_count: usize,
    pub people: Vec<String>,
    pub organizations: Vec<String>,
    pub ambiguous_people: Vec<String>,
    pub suggested_notes_folder: String,
    pub suggested_people_folder: String,
    pub suggested_organizations_folder: String,
    pub folder_candidates: Vec<String>,
    pub sample_titles: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GranolaImportOptions {
    pub notes_folder: String,
    pub people_folder: String,
    pub organizations_folder: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GranolaImportResult {
    pub imported_count: usize,
    pub note_paths: Vec<String>,
    pub people_created: usize,
    pub organizations_created: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Person {
    pub name: String,
    pub email: Option<String>,
    pub organizations: Vec<String>,
    /// False when source and vault evidence cannot safely identify one person.
    pub resolved: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Meeting {
    /// Source-native identity used only while reading the export/API response.
    pub id: Option<String>,
    pub title: String,
    pub created_at: Option<String>,
    pub notes: Option<String>,
    pub transcript: Option<String>,
    pub people: Vec<Person>,
    pub organizations: Vec<String>,
    /// Ephemeral path/URL evidence used only to recover an event timestamp.
    pub provenance: Option<String>,
    /// Allows the product UI to explain a missing remote transcript without
    /// changing the native note body.
    pub plan_gated_transcript: bool,
}

pub struct GranolaImporter {
    meetings: Vec<Meeting>,
    warnings: Vec<String>,
    vault_root: PathBuf,
    file_count: usize,
    inbox_folder: String,
    people_folder: String,
}

impl GranolaImporter {
    fn new(
        meetings: Vec<Meeting>,
        warnings: Vec<String>,
        vault_root: &Path,
        file_count: usize,
        inbox_folder: &str,
        people_folder: &str,
    ) -> Self {
        Self {
            meetings,
            warnings,
            vault_root: vault_root.to_path_buf(),
            file_count,
            inbox_folder: inbox_folder.to_string(),
            people_folder: people_folder.to_string(),
        }
    }
}

impl Importer for GranolaImporter {
    type Survey = GranolaImportSurvey;
    type Options = GranolaImportOptions;
    type Output = GranolaImportResult;

    fn survey(&self) -> AnyResult<Self::Survey> {
        let ambiguous_people = identity_ambiguities(&self.meetings)
            .into_iter()
            .collect::<Vec<_>>();
        let people = self
            .meetings
            .iter()
            .flat_map(|meeting| &meeting.people)
            .map(|person| person.name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let organizations = self
            .meetings
            .iter()
            .flat_map(|meeting| &meeting.organizations)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let candidates = folder_candidates(&self.vault_root);
        Ok(GranolaImportSurvey {
            file_count: self.file_count,
            meeting_count: self.meetings.len(),
            people,
            organizations,
            ambiguous_people,
            suggested_notes_folder: folder_or_default(&self.inbox_folder, "meetings"),
            suggested_people_folder: folder_or_default(&self.people_folder, "people"),
            suggested_organizations_folder: choose_folder(
                &candidates,
                &["organizations", "orgs", "companies"],
            )
            .unwrap_or_else(|| "organizations".to_string()),
            folder_candidates: candidates,
            sample_titles: self
                .meetings
                .iter()
                .take(4)
                .map(|meeting| meeting.title.clone())
                .collect(),
            warnings: self.warnings.clone(),
        })
    }

    fn import(mut self, options: &Self::Options) -> AnyResult<Self::Output> {
        if self.meetings.is_empty() {
            bail!("No Granola meetings found in the selected source");
        }
        stage_and_commit_import(&mut self.meetings, self.warnings, &self.vault_root, options)
    }
}

pub fn survey(
    paths: &[String],
    vault_root: &Path,
    inbox_folder: &str,
    people_folder: &str,
) -> Result<GranolaImportSurvey, String> {
    let (meetings, warnings) = read_meetings(paths)?;
    GranolaImporter::new(
        meetings,
        warnings,
        vault_root,
        paths.len(),
        inbox_folder,
        people_folder,
    )
    .survey()
    .map_err(|error| error.to_string())
}

pub fn import(
    paths: &[String],
    vault_root: &Path,
    options: &GranolaImportOptions,
) -> Result<GranolaImportResult, String> {
    let (meetings, warnings) = read_meetings(paths)?;
    GranolaImporter::new(meetings, warnings, vault_root, paths.len(), "", "")
        .import(options)
        .map_err(|error| error.to_string())
}

pub fn import_meetings(
    meetings: Vec<Meeting>,
    warnings: Vec<String>,
    vault_root: &Path,
    options: &GranolaImportOptions,
) -> Result<GranolaImportResult, String> {
    GranolaImporter::new(meetings, warnings, vault_root, 0, "", "")
        .import(options)
        .map_err(|error| error.to_string())
}

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
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactKind {
    Meeting,
    Person,
    Organization,
}

struct StagedArtifact {
    relative_path: PathBuf,
    contents: String,
    kind: ArtifactKind,
}

fn stage_and_commit_import(
    meetings: &mut Vec<Meeting>,
    mut warnings: Vec<String>,
    vault_root: &Path,
    options: &GranolaImportOptions,
) -> AnyResult<GranolaImportResult> {
    std::fs::create_dir_all(vault_root)
        .with_context(|| format!("creating vault {}", vault_root.display()))?;
    let notes_dir = confined_vault_folder(vault_root, &options.notes_folder)?;
    let people_dir = confined_vault_folder(vault_root, &options.people_folder)?;
    let organizations_dir = confined_vault_folder(vault_root, &options.organizations_folder)?;
    for directory in [&notes_dir, &people_dir, &organizations_dir] {
        reject_symlinked_ancestors(vault_root, directory)?;
    }

    apply_identity_integrity(meetings, &people_dir, &mut warnings);
    meetings.sort_by(|left, right| {
        meeting_occurred_at(left)
            .cmp(&meeting_occurred_at(right))
            .then_with(|| {
                left.title
                    .to_ascii_lowercase()
                    .cmp(&right.title.to_ascii_lowercase())
            })
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut reserved = BTreeSet::new();
    for directory in [&notes_dir, &people_dir, &organizations_dir] {
        reserve_directory_entries(directory, &mut reserved);
    }
    let mut people_links = BTreeMap::new();
    let mut organization_links = BTreeMap::new();
    let mut artifacts = Vec::new();

    let mut people = BTreeMap::<String, Person>::new();
    for person in meetings
        .iter()
        .flat_map(|meeting| &meeting.people)
        .filter(|person| person.resolved)
    {
        people
            .entry(person_identity_key(person))
            .or_insert_with(|| person.clone());
    }
    for (identity, person) in people {
        let desired = people_dir.join(format!("{}.md", file_stem(&person.name)));
        let destination = if existing_regular_file(&desired) {
            reserve_existing(&desired, &mut reserved);
            desired
        } else {
            allocate_destination(&people_dir, &file_stem(&person.name), &mut reserved)
        };
        people_links.insert(identity, destination_wikilink(&destination));
        if !existing_regular_file(&destination) {
            artifacts.push(StagedArtifact {
                relative_path: vault_relative(vault_root, &destination)?,
                contents: native_person_markdown(
                    &person,
                    &meeting_date_for_person(meetings, &person),
                ),
                kind: ArtifactKind::Person,
            });
        }
    }

    let organizations = meetings
        .iter()
        .flat_map(|meeting| &meeting.organizations)
        .cloned()
        .collect::<BTreeSet<_>>();
    for organization in organizations {
        let desired = organizations_dir.join(format!("{}.md", file_stem(&organization)));
        let destination = if existing_regular_file(&desired) {
            reserve_existing(&desired, &mut reserved);
            desired
        } else {
            allocate_destination(&organizations_dir, &file_stem(&organization), &mut reserved)
        };
        organization_links.insert(
            organization.trim().to_ascii_lowercase(),
            destination_wikilink(&destination),
        );
        if !existing_regular_file(&destination) {
            artifacts.push(StagedArtifact {
                relative_path: vault_relative(vault_root, &destination)?,
                contents: native_organization_markdown(
                    &organization,
                    &meeting_date_for_organization(meetings, &organization),
                ),
                kind: ArtifactKind::Organization,
            });
        }
    }

    for meeting in meetings.iter() {
        let stem = format!("{} {}", meeting_date(meeting), file_stem(&meeting.title));
        let destination = allocate_destination(&notes_dir, &stem, &mut reserved);
        artifacts.push(StagedArtifact {
            relative_path: vault_relative(vault_root, &destination)?,
            contents: native_meeting_markdown(meeting, &people_links, &organization_links),
            kind: ArtifactKind::Meeting,
        });
    }

    artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let staging = tempfile::Builder::new()
        .prefix(".granola-import-")
        .tempdir_in(vault_root)
        .context("creating temporary Granola import staging directory")?;
    write_and_validate_staging(staging.path(), &artifacts)?;
    let committed = commit_staged_files(staging.path(), vault_root, &artifacts)?;

    let mut note_paths = Vec::new();
    let mut people_created = 0;
    let mut organizations_created = 0;
    for (artifact, path) in artifacts.iter().zip(committed) {
        match artifact.kind {
            ArtifactKind::Meeting => note_paths.push(path.to_string_lossy().to_string()),
            ArtifactKind::Person => people_created += 1,
            ArtifactKind::Organization => organizations_created += 1,
        }
    }

    Ok(GranolaImportResult {
        imported_count: note_paths.len(),
        note_paths,
        people_created,
        organizations_created,
        warnings,
    })
}

fn native_meeting_markdown(
    meeting: &Meeting,
    people_links: &BTreeMap<String, String>,
    organization_links: &BTreeMap<String, String>,
) -> String {
    let resolved_people = meeting
        .people
        .iter()
        .filter(|person| person.resolved)
        .filter_map(|person| people_links.get(&person_identity_key(person)))
        .map(|link| format!("[[{link}]]"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let organizations = meeting
        .organizations
        .iter()
        .filter_map(|organization| {
            organization_links.get(&organization.trim().to_ascii_lowercase())
        })
        .map(|link| format!("[[{link}]]"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!(
        "occurred_at: {}\n",
        meeting_occurred_at(meeting).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    ));
    if !resolved_people.is_empty() {
        yaml_list(&mut out, "people", &resolved_people);
    }
    if !organizations.is_empty() {
        yaml_list(&mut out, "organizations", &organizations);
    }
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", meeting.title));
    out.push_str("## Notes\n\n");
    out.push_str(
        meeting
            .notes
            .as_deref()
            .unwrap_or("_No notes were present in the export._"),
    );
    out.push_str("\n\n## Transcript\n\n");
    out.push_str(
        meeting
            .transcript
            .as_deref()
            .unwrap_or("_No transcript was present in the export._"),
    );
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn person_identity_key(person: &Person) -> String {
    person
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| person.name.trim().to_ascii_lowercase())
}

fn meeting_date_for_person(meetings: &[Meeting], wanted: &Person) -> String {
    meetings
        .iter()
        .filter(|meeting| {
            meeting
                .people
                .iter()
                .any(|person| person_identity_key(person) == person_identity_key(wanted))
        })
        .map(meeting_date)
        .min()
        .unwrap_or_else(|| "1970-01-01".to_string())
}

fn meeting_date_for_organization(meetings: &[Meeting], wanted: &str) -> String {
    meetings
        .iter()
        .filter(|meeting| {
            meeting
                .organizations
                .iter()
                .any(|organization| organization.eq_ignore_ascii_case(wanted))
        })
        .map(meeting_date)
        .min()
        .unwrap_or_else(|| "1970-01-01".to_string())
}

fn reserve_existing(path: &Path, reserved: &mut BTreeSet<String>) {
    reserved.insert(path.to_string_lossy().to_ascii_lowercase());
}

fn reserve_directory_entries(directory: &Path, reserved: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        reserve_existing(&entry.path(), reserved);
    }
}

fn existing_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

fn allocate_destination(directory: &Path, stem: &str, reserved: &mut BTreeSet<String>) -> PathBuf {
    for ordinal in 1usize.. {
        let filename = if ordinal == 1 {
            format!("{stem}.md")
        } else {
            format!("{stem} ({ordinal}).md")
        };
        let candidate = directory.join(filename);
        let key = candidate.to_string_lossy().to_ascii_lowercase();
        if !candidate.exists() && reserved.insert(key) {
            return candidate;
        }
    }
    unreachable!("unbounded collision suffix space")
}

fn destination_wikilink(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Untitled")
        .to_string()
}

fn vault_relative(vault_root: &Path, path: &Path) -> AnyResult<PathBuf> {
    path.strip_prefix(vault_root)
        .with_context(|| format!("import destination escaped vault: {}", path.display()))
        .map(Path::to_path_buf)
}

fn write_and_validate_staging(stage_root: &Path, artifacts: &[StagedArtifact]) -> AnyResult<()> {
    let mut seen = BTreeSet::new();
    for artifact in artifacts {
        validate_relative_path(&artifact.relative_path)?;
        let key = artifact
            .relative_path
            .to_string_lossy()
            .to_ascii_lowercase();
        if !seen.insert(key) {
            bail!(
                "duplicate staged Granola destination: {}",
                artifact.relative_path.display()
            );
        }
        let staged_path = stage_root.join(&artifact.relative_path);
        if let Some(parent) = staged_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&staged_path, artifact.contents.as_bytes())?;
        let mut staged = Vec::new();
        std::fs::File::open(&staged_path)?.read_to_end(&mut staged)?;
        if staged != artifact.contents.as_bytes() {
            bail!("staged Granola artifact failed validation");
        }
    }
    Ok(())
}

fn commit_staged_files(
    stage_root: &Path,
    vault_root: &Path,
    artifacts: &[StagedArtifact],
) -> AnyResult<Vec<PathBuf>> {
    for artifact in artifacts {
        let final_path = vault_root.join(&artifact.relative_path);
        if final_path.exists() {
            bail!(
                "Granola import destination appeared during staging: {}",
                final_path.display()
            );
        }
        if let Some(parent) = final_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let mut committed = Vec::new();
    for artifact in artifacts {
        let staged_path = stage_root.join(&artifact.relative_path);
        let final_path = vault_root.join(&artifact.relative_path);
        let copy_result = (|| -> std::io::Result<()> {
            let mut source = std::fs::File::open(&staged_path)?;
            let mut destination = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&final_path)?;
            committed.push(final_path.clone());
            std::io::copy(&mut source, &mut destination)?;
            destination.flush()?;
            Ok(())
        })();
        if let Err(error) = copy_result {
            for path in committed.iter().rev() {
                let _ = std::fs::remove_file(path);
            }
            return Err(error).with_context(|| {
                format!("committing staged Granola file {}", final_path.display())
            });
        }
    }
    Ok(committed)
}

fn validate_relative_path(path: &Path) -> AnyResult<()> {
    use std::path::Component;
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!(
            "Granola staging path is not vault-relative: {}",
            path.display()
        );
    }
    Ok(())
}

fn reject_symlinked_ancestors(vault_root: &Path, destination: &Path) -> AnyResult<()> {
    let relative = vault_relative(vault_root, destination)?;
    let mut current = vault_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => bail!(
                "Granola import folder cannot traverse a symlink: {}",
                current.display()
            ),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", current.display()))
            }
        }
    }
    Ok(())
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
            Ok(mut found) if !found.is_empty() => {
                let source_path = std::fs::canonicalize(path)
                    .unwrap_or_else(|_| PathBuf::from(path))
                    .to_string_lossy()
                    .to_string();
                for meeting in &mut found {
                    meeting.provenance = Some(source_path.clone());
                }
                meetings.append(&mut found)
            }
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
    let provenance = text_field(
        value,
        &[
            "url",
            "link",
            "meeting_url",
            "meetingUrl",
            "notes_url",
            "notesUrl",
        ],
    )
    .or_else(|| {
        id.as_ref()
            .map(|id| format!("https://notes.granola.ai/t/{id}"))
    });
    Meeting {
        id,
        title,
        created_at,
        notes,
        transcript,
        people,
        organizations,
        provenance,
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
            provenance: None,
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
            resolved: true,
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
            resolved: true,
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

fn identity_ambiguities(meetings: &[Meeting]) -> BTreeSet<String> {
    let mut evidence = BTreeMap::<String, BTreeSet<String>>::new();
    let mut display = BTreeMap::<String, String>::new();
    for person in meetings.iter().flat_map(|meeting| &meeting.people) {
        let name_key = person.name.trim().to_ascii_lowercase();
        display
            .entry(name_key.clone())
            .or_insert_with(|| person.name.trim().to_string());
        let identity_key = person
            .email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
            .unwrap_or_else(|| "<name-only>".to_string());
        evidence.entry(name_key).or_default().insert(identity_key);
    }
    evidence
        .into_iter()
        .filter_map(|(key, identities)| {
            let bare_name_only = identities.len() == 1
                && identities.contains("<name-only>")
                && key.split_whitespace().count() < 2;
            (identities.len() > 1 || bare_name_only).then(|| display.remove(&key).unwrap_or(key))
        })
        .collect()
}

fn apply_identity_integrity(
    meetings: &mut [Meeting],
    people_dir: &Path,
    warnings: &mut Vec<String>,
) {
    let ambiguous = identity_ambiguities(meetings)
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let mut warned = BTreeSet::new();
    for person in meetings
        .iter_mut()
        .flat_map(|meeting| meeting.people.iter_mut())
    {
        let name_key = person.name.trim().to_ascii_lowercase();
        let mut reason = ambiguous
            .contains(&name_key)
            .then(|| "ambiguous source identity".to_string());
        let existing_path = people_dir.join(format!("{}.md", file_stem(&person.name)));
        if reason.is_none() && existing_path.exists() {
            if !existing_regular_file(&existing_path) {
                reason = Some("existing person path is not a regular vault file".to_string());
            } else {
                if let (Some(incoming), Ok(existing)) = (
                    person.email.as_deref(),
                    std::fs::read_to_string(&existing_path),
                ) {
                    if let Some(stored) = frontmatter_value(&existing, "email") {
                        if !stored.eq_ignore_ascii_case(incoming) {
                            reason = Some(format!(
                                "existing person note has a different email ({stored})"
                            ));
                        }
                    }
                }
            }
        }
        if let Some(reason) = reason {
            person.resolved = false;
            if warned.insert(name_key) {
                warnings.push(format!(
                    "Omitted person backlink for {}: {reason}; identity requires approval",
                    person.name
                ));
            }
        }
    }
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
            resolved: true,
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
    for key in ["organizations", "companies"] {
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
    if let Some(items) = value.get("entities").and_then(Value::as_array) {
        for item in items {
            let entity_kind = text_field(item, &["type", "kind"])
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(entity_kind.as_str(), "company" | "organization" | "org") {
                if let Some(name) =
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
    dedupe_strings_case_insensitive(
        people
            .iter()
            .flat_map(|person| person.organizations.iter().cloned())
            .collect(),
    )
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

fn native_person_markdown(person: &Person, created_date: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str("type: person\n");
    out.push_str("tags: [people]\n");
    out.push_str(&format!("created: {}\n", yaml_escape(created_date)));
    if let Some(email) = &person.email {
        out.push_str(&format!("email: {}\n", yaml_escape(email)));
    }
    let organizations = person_organizations(person);
    if !organizations.is_empty() {
        yaml_list(&mut out, "organizations", &organizations);
    }
    out.push_str("---\n");
    out.push_str(&format!("# {}\n", person.name));
    out
}

fn native_organization_markdown(organization: &str, created_date: &str) -> String {
    format!(
        "---\ntype: organization\ntags: [organizations]\ncreated: {}\n---\n# {}\n",
        yaml_escape(created_date),
        organization
    )
}

fn person_organizations(person: &Person) -> Vec<String> {
    organizations_from_people(std::slice::from_ref(person))
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

fn confined_vault_folder(vault_root: &Path, value: &str) -> AnyResult<PathBuf> {
    use std::path::Component;

    if Path::new(value.trim()).is_absolute() {
        bail!("integration folder must stay inside the vault: {value}");
    }
    let cleaned = clean_folder(value);
    if cleaned.is_empty() {
        bail!("integration folder cannot be empty");
    }
    let relative = Path::new(&cleaned);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        bail!("integration folder must stay inside the vault: {value}");
    }
    Ok(vault_root.join(relative))
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

fn meeting_date(meeting: &Meeting) -> String {
    meeting
        .created_at
        .as_deref()
        .and_then(date_prefix)
        .unwrap_or_else(|| meeting_occurred_at(meeting).format("%Y-%m-%d").to_string())
}

fn meeting_occurred_at(meeting: &Meeting) -> DateTime<Utc> {
    meeting
        .created_at
        .as_deref()
        .and_then(parse_source_datetime)
        .or_else(|| {
            let path = meeting.provenance.as_deref()?;
            std::fs::metadata(path)
                .ok()?
                .modified()
                .ok()
                .map(DateTime::<Utc>::from)
        })
        .unwrap_or_else(|| DateTime::<Utc>::from(std::time::SystemTime::UNIX_EPOCH))
}

fn parse_local_datetime(value: &str) -> Option<DateTime<Local>> {
    parse_source_datetime(value).map(|dt| dt.with_timezone(&Local))
}

pub(crate) fn parse_source_datetime(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Some(value.with_timezone(&Utc));
    }
    if let Some(date) = ["%b %e, %Y", "%B %e, %Y"]
        .into_iter()
        .find_map(|format| NaiveDate::parse_from_str(value.trim(), format).ok())
    {
        return date
            .and_hms_opt(0, 0, 0)
            .map(|value| Utc.from_utc_datetime(&value));
    }

    const ZONES: [(&str, i32); 10] = [
        ("UTC", 0),
        ("GMT", 0),
        ("EST", -5 * 3600),
        ("EDT", -4 * 3600),
        ("CST", -6 * 3600),
        ("CDT", -5 * 3600),
        ("MST", -7 * 3600),
        ("MDT", -6 * 3600),
        ("PST", -8 * 3600),
        ("PDT", -7 * 3600),
    ];
    for (zone, seconds) in ZONES {
        let Some(local_value) = value.trim().strip_suffix(zone).map(str::trim) else {
            continue;
        };
        let naive = ["%b %e, %Y %I:%M %p", "%B %e, %Y %I:%M %p"]
            .into_iter()
            .find_map(|format| NaiveDateTime::parse_from_str(local_value, format).ok())?;
        let offset = FixedOffset::east_opt(seconds)?;
        return offset
            .from_local_datetime(&naive)
            .single()
            .map(|value| value.with_timezone(&Utc));
    }
    None
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

fn yaml_list(out: &mut String, key: &str, values: &[String]) {
    out.push_str(key);
    out.push_str(":\n");
    for value in values {
        out.push_str(&format!("  - \"{}\"\n", yaml_escape(value)));
    }
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

    const GRANOLA_JSON: &str = r#"{
  "documents": [{
    "id": "grn_001",
    "title": "Pilot scope review",
    "created_at": "2026-06-24T15:30:00Z",
    "enhanced_notes": "Discussed launch scope and owner handoffs.",
    "people": {"attendees": [
      {"name": "Alex Chen", "email": "alex@example.com", "company": "Acme Labs"},
      {"name": "Dana Lee", "email": "dana@icloud.com"}
    ]},
    "entities": [
      {"name": "Northstar", "type": "company"},
      {"name": "Not An Org", "type": "topic"}
    ],
    "transcript_segments": [
      {"speaker_name": "Alex Chen", "start": "00:17", "text": "Acme Labs can own the rollout."}
    ]
  }]
}"#;

    const AMBIGUOUS_JSON: &str = r#"{"documents":[
      {"id":"one","title":"Kevin one","created_at":"2026-07-05T09:00:00Z","notes":"First Kevin.","attendees":[{"name":"Kevin Park","email":"kevin.one@example.com"}]},
      {"id":"two","title":"Kevin two","created_at":"2026-07-06T09:00:00Z","notes":"Second Kevin.","attendees":[{"name":"Kevin Park","email":"kevin.two@example.com"}]}
    ]}"#;

    fn options() -> GranolaImportOptions {
        GranolaImportOptions {
            notes_folder: "meetings".to_string(),
            people_folder: "people".to_string(),
            organizations_folder: "organizations".to_string(),
        }
    }

    fn write_fixture(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn import_fixture(root: &Path, fixture: &Path) -> GranolaImportResult {
        import(&[fixture.to_string_lossy().to_string()], root, &options()).unwrap()
    }

    fn staging_directories(root: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(root)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".granola-import-"))
            })
            .collect()
    }

    #[test]
    fn survey_is_in_memory_and_reports_ambiguous_people() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "ambiguous.json", AMBIGUOUS_JSON);

        let report = survey(
            &[fixture.to_string_lossy().to_string()],
            dir.path(),
            "meetings",
            "people",
        )
        .unwrap();

        assert_eq!(report.meeting_count, 2);
        assert_eq!(report.ambiguous_people, vec!["Kevin Park"]);
        assert!(!dir.path().join(".margins").exists());
        assert!(staging_directories(dir.path()).is_empty());
    }

    #[test]
    fn import_emits_only_native_shape_and_no_persistent_granola_state() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);

        let result = import_fixture(dir.path(), &fixture);

        assert_eq!(result.imported_count, 1);
        assert_eq!(result.people_created, 2);
        assert_eq!(result.organizations_created, 2);
        let note = std::fs::read_to_string(&result.note_paths[0]).unwrap();
        assert!(note.starts_with("---\noccurred_at: 2026-06-24T15:30:00Z\npeople:\n"));
        assert!(note.contains("# Pilot scope review\n\n## Notes\n\nDiscussed launch scope"));
        assert!(
            note.contains("## Transcript\n\n[00:17] **Alex Chen:** Acme Labs can own the rollout.")
        );
        for forbidden in [
            "source:",
            "granola_id:",
            "source_account:",
            "source_id:",
            "margins_session:",
            "participants_unresolved:",
            "title:",
            "content_hash:",
            "imported_at:",
        ] {
            assert!(!note.contains(forbidden), "found {forbidden} in {note}");
        }
        assert!(!dir.path().join(".margins/integrations").exists());
        assert!(!dir.path().join(".margins/sessions.sqlite").exists());
        assert!(staging_directories(dir.path()).is_empty());
    }

    #[test]
    fn ambiguous_same_name_different_email_is_omitted_without_evidence_leak() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "ambiguous.json", AMBIGUOUS_JSON);

        let result = import_fixture(dir.path(), &fixture);

        assert_eq!(result.imported_count, 2);
        assert!(!dir.path().join("people/Kevin Park.md").exists());
        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("Kevin Park")));
        for path in result.note_paths {
            let note = std::fs::read_to_string(path).unwrap();
            assert!(!note.contains("[[Kevin Park]]"));
            assert!(!note.contains("kevin.one@example.com"));
            assert!(!note.contains("kevin.two@example.com"));
            assert!(!note.contains("participants_unresolved"));
        }
    }

    #[test]
    fn organizations_require_explicit_export_evidence_not_email_domains() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);

        let result = import_fixture(dir.path(), &fixture);
        let note = std::fs::read_to_string(&result.note_paths[0]).unwrap();

        assert!(note.contains("[[Acme Labs]]"));
        assert!(note.contains("[[Northstar]]"));
        assert!(!note.contains("[[Example]]"));
        assert!(!note.contains("[[Icloud]]"));
        assert!(!note.contains("Not An Org"));
        assert!(!dir.path().join("organizations/Example.md").exists());
        assert!(!dir.path().join("organizations/Icloud.md").exists());
    }

    #[test]
    fn deterministic_collision_allocation_preserves_user_files_and_batch_items() {
        let dir = tempfile::tempdir().unwrap();
        let meetings_dir = dir.path().join("meetings");
        std::fs::create_dir_all(&meetings_dir).unwrap();
        let existing = meetings_dir.join("2026-08-01 Same title.md");
        std::fs::write(&existing, "user-owned\n").unwrap();
        let fixture = write_fixture(
            dir.path(),
            "collisions.json",
            r#"{"documents":[
              {"id":"b","title":"Same title","created_at":"2026-08-01T10:00:00Z","notes":"Second"},
              {"id":"a","title":"Same title","created_at":"2026-08-01T09:00:00Z","notes":"First"}
            ]}"#,
        );

        let result = import_fixture(dir.path(), &fixture);

        assert_eq!(std::fs::read_to_string(existing).unwrap(), "user-owned\n");
        assert_eq!(
            result
                .note_paths
                .iter()
                .map(|path| Path::new(path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string())
                .collect::<Vec<_>>(),
            vec![
                "2026-08-01 Same title (2).md",
                "2026-08-01 Same title (3).md"
            ]
        );
        assert!(std::fs::read_to_string(&result.note_paths[0])
            .unwrap()
            .contains("First"));
        assert!(std::fs::read_to_string(&result.note_paths[1])
            .unwrap()
            .contains("Second"));
    }

    #[test]
    fn sanitized_same_batch_people_collisions_get_distinct_links() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(
            dir.path(),
            "people-collisions.json",
            r#"{"title":"Collision people","created_at":"2026-08-02T09:00:00Z","notes":"Names collide after sanitizing.","attendees":[{"name":"Dana/Lee","email":"slash@example.com"},{"name":"Dana:Lee","email":"colon@example.com"}]}"#,
        );

        let result = import_fixture(dir.path(), &fixture);
        let note = std::fs::read_to_string(&result.note_paths[0]).unwrap();

        assert!(dir.path().join("people/Dana-Lee.md").exists());
        assert!(dir.path().join("people/Dana-Lee (2).md").exists());
        assert!(note.contains("[[Dana-Lee]]"));
        assert!(note.contains("[[Dana-Lee (2)]]"));
    }

    #[test]
    fn staging_is_removed_when_commit_preflight_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("blocked"), "not a directory").unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);
        let mut invalid = options();
        invalid.notes_folder = "blocked/meetings".to_string();

        let error = import(
            &[fixture.to_string_lossy().to_string()],
            dir.path(),
            &invalid,
        )
        .unwrap_err();

        assert!(
            error.contains("blocked") || error.contains("directory"),
            "{error}"
        );
        assert!(staging_directories(dir.path()).is_empty());
        assert!(!dir.path().join("people/Alex Chen.md").exists());
        assert!(!dir.path().join("organizations/Acme Labs.md").exists());
    }

    #[test]
    fn unsafe_folders_are_rejected_without_staging() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);
        let mut invalid = options();
        invalid.notes_folder = "../outside".to_string();

        assert!(import(
            &[fixture.to_string_lossy().to_string()],
            dir.path(),
            &invalid,
        )
        .is_err());
        assert!(staging_directories(dir.path()).is_empty());
    }

    #[test]
    fn existing_person_with_conflicting_email_is_not_linked_or_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let people_dir = dir.path().join("people");
        std::fs::create_dir_all(&people_dir).unwrap();
        let person_path = people_dir.join("Alex Chen.md");
        let custom = "---\nemail: other@example.com\n---\ncustom\n";
        std::fs::write(&person_path, custom).unwrap();
        let fixture = write_fixture(dir.path(), "granola.json", GRANOLA_JSON);

        let result = import_fixture(dir.path(), &fixture);
        let note = std::fs::read_to_string(&result.note_paths[0]).unwrap();

        assert!(!note.contains("[[Alex Chen]]"));
        assert_eq!(std::fs::read_to_string(person_path).unwrap(), custom);
    }

    #[test]
    fn mcp_human_timestamp_preserves_underlying_event_time() {
        let meeting = meeting_from_value(&serde_json::json!({
            "id": "mcp-human-date",
            "title": "MCP date fixture",
            "created_at": "Jun 4, 2026 12:45 PM EDT",
            "summary": "A real-shaped MCP timestamp"
        }));
        assert_eq!(meeting_date(&meeting), "2026-06-04");
        assert_eq!(
            meeting_occurred_at(&meeting).to_rfc3339(),
            "2026-06-04T16:45:00+00:00"
        );
    }

    #[test]
    fn jsonl_and_csv_sources_remain_supported() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = write_fixture(
            dir.path(),
            "granola.jsonl",
            "{\"title\":\"Line one\",\"created_at\":\"2026-06-25T10:00:00Z\",\"notes\":\"First\"}\n{\"title\":\"Line two\",\"created_at\":\"2026-06-26T10:00:00Z\",\"notes\":\"Second\"}\n",
        );
        assert_eq!(import_fixture(dir.path(), &jsonl).imported_count, 2);

        let csv = write_fixture(
            dir.path(),
            "granola.csv",
            "title,created_at,notes,transcript,company\nCSV import,2026-06-27T12:00:00Z,CSV notes,CSV transcript,WidgetCo\n",
        );
        let result = import_fixture(dir.path(), &csv);
        assert_eq!(result.imported_count, 1);
        assert!(std::fs::read_to_string(&result.note_paths[0])
            .unwrap()
            .contains("[[WidgetCo]]"));
    }

    #[test]
    fn missing_source_sections_use_generic_native_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = write_fixture(
            dir.path(),
            "missing.json",
            r#"{"title":"Sparse meeting","created_at":"2026-07-02T09:00:00Z","attendees":[]}"#,
        );

        let result = import_fixture(dir.path(), &fixture);
        let note = std::fs::read_to_string(&result.note_paths[0]).unwrap();

        assert!(note.contains("## Notes\n\n_No notes were present in the export._"));
        assert!(note.contains("## Transcript\n\n_No transcript was present in the export._"));
        assert!(!note.contains("Granola"));
    }
}
