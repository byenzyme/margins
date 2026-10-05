//! The Workspace program: `$MARGINS_HOME/configs/<id>.enzyme`.
//!
//! A Workspace's whole configuration is one `.enzyme` program containing one
//! `workspace "<id>" { … }` block, parsed and rendered by `enzyme-spec`. The
//! program is the source of truth. [`derive_view`] computes the read-only typed
//! [`WorkspaceConfig`] view the rest of Margins reads (bindings, Home, policy,
//! selectors). Mutations never write the view back: [`reconcile`] edits the
//! program AST toward a desired view while preserving every language feature
//! the view cannot express (learning settings, custom profiles, `when asked`,
//! create-note guidance, name patterns, …).

use crate::workspace::{
    CalendarCollectionSelector, GmailCollectionSelector, GranolaCollectionSelector,
    GranolaTimeRange, RetentionPolicy, SourceRole, WorkspaceBinding, WorkspaceConfig,
    WorkspaceEntity, WorkspaceEntityOptions, WorkspacePolicy,
};
use anyhow::{bail, ensure, Context, Result};
use enzyme_spec::{
    HostField, HostSource, HostValue, Learning, MarkdownSource, NotePolicy, Program, Reading,
    Source, SqliteSource, SqliteWho,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const SOURCE_MARKDOWN: &str = "markdown";
pub const SOURCE_CAPTURES: &str = "margins-captures";
pub const SOURCE_GOOGLE_MAIL: &str = "google-mail";
pub const SOURCE_GOOGLE_CALENDAR: &str = "google-calendar";
pub const SOURCE_GOOGLE_MEET: &str = "google-meet";
pub const SOURCE_GRANOLA: &str = "granola";

const FIELD_PATH: &str = "path";
const FIELD_ACCOUNT: &str = "account";
const FIELD_QUERY: &str = "query";
const FIELD_BACKFILL_DAYS: &str = "backfill days";
const FIELD_LOOKBACK_DAYS: &str = "lookback days";
const FIELD_LOOKAHEAD_DAYS: &str = "lookahead days";
const FIELD_TIME_RANGE: &str = "time range";
const FIELD_WORKSPACE_ONLY: &str = "workspace only";

/// One parsed Workspace program and the exact text it was parsed from.
///
/// Equality is textual: the revision of a Workspace is the SHA-256 of these
/// bytes, so two programs are the same configuration only when their bytes are.
#[derive(Debug, Clone)]
pub struct WorkspaceProgram {
    text: String,
    program: Program,
}

impl PartialEq for WorkspaceProgram {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for WorkspaceProgram {}

impl WorkspaceProgram {
    /// Parse a program file. It must contain exactly one `workspace` block and
    /// may define reusable `profile`s; machine settings, global learning, global
    /// `when asked` and `vault` blocks belong elsewhere.
    pub fn parse(text: &str) -> Result<Self> {
        let program = enzyme_spec::parse(text)?;
        ensure!(
            program.workspaces.len() == 1,
            "a Workspace program must contain exactly one workspace block, found {}",
            program.workspaces.len()
        );
        ensure!(
            program.vaults.is_empty(),
            "a Workspace program cannot contain vault blocks"
        );
        ensure!(
            program.settings == enzyme_spec::Settings::default(),
            "a Workspace program cannot contain machine settings"
        );
        ensure!(
            program.learning == Learning::default(),
            "put learning settings inside the workspace block"
        );
        ensure!(
            program.retrieval.is_none(),
            "put `when asked` inside the workspace block"
        );
        Ok(Self {
            text: text.to_string(),
            program,
        })
    }

    /// Render an AST canonically through `enzyme_spec::render_program`.
    pub fn from_program(program: Program) -> Result<Self> {
        let text = enzyme_spec::render_program(&program);
        // Reparse so the stored AST is exactly what the bytes say.
        Self::parse(&text).context("rendered Workspace program does not parse")
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn program(&self) -> &Program {
        &self.program
    }

    pub fn workspace(&self) -> &enzyme_spec::Workspace {
        &self.program.workspaces[0]
    }

    pub fn id(&self) -> &str {
        &self.workspace().name
    }

    /// The Workspace revision: SHA-256 of the program bytes.
    pub fn sha256(&self) -> String {
        program_sha256(&self.text)
    }
}

pub fn program_sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn user_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Expand a leading `~` the same way `enzyme_spec::resolve` does.
pub fn expand_path(path: &str) -> PathBuf {
    let home = user_home();
    if path == "~" {
        home
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Program -> view
// ---------------------------------------------------------------------------

/// Compute the typed, read-only view of a program. `name` and `retention` come
/// from machine configuration, not from the program.
pub fn derive_view(
    program: &WorkspaceProgram,
    name: Option<String>,
    retention: RetentionPolicy,
) -> Result<WorkspaceConfig> {
    let workspace = program.workspace();
    let (home_source, note_folder) = home_policy(workspace)?;
    let mut bindings = BTreeMap::new();
    for source in &workspace.sources {
        let name = source.name();
        let binding = match source {
            Source::Markdown(markdown) => {
                let role = if markdown.name.eq_ignore_ascii_case(&home_source) {
                    SourceRole::Home
                } else {
                    SourceRole::Reference
                };
                WorkspaceBinding::NativeMarkdown {
                    path: expand_path(&markdown.path),
                    role,
                    note_folder: (role == SourceRole::Home)
                        .then(|| note_folder.clone())
                        .flatten(),
                }
            }
            Source::Host(host) => host_binding(host)?,
            Source::Sqlite(_) | Source::Arena(_) => bail!(
                "source {} {name:?}: Margins Workspaces declare markdown, {SOURCE_CAPTURES}, {SOURCE_GOOGLE_MAIL}, {SOURCE_GOOGLE_CALENDAR}, {SOURCE_GOOGLE_MEET}, or {SOURCE_GRANOLA} sources",
                source.kind()
            ),
        };
        ensure!(
            bindings.insert(name.to_string(), binding).is_none(),
            "duplicate source {name:?}"
        );
    }
    Ok(WorkspaceConfig {
        id: workspace.name.clone(),
        name,
        policy: view_policy(workspace),
        retention,
        bindings,
    })
}

/// The Markdown source and folder named by the one create-note policy.
fn home_policy(workspace: &enzyme_spec::Workspace) -> Result<(String, Option<PathBuf>)> {
    let policy = match workspace.note_policies.as_slice() {
        [policy] => policy,
        [] => bail!(
            "workspace {:?} must declare where Margins writes notes: add exactly one `remember in folder \"<folder>\" create note` (\".\" is the source root)",
            workspace.name
        ),
        _ => bail!(
            "workspace {:?} declares {} create-note policies; a Margins Workspace writes into exactly one folder",
            workspace.name,
            workspace.note_policies.len()
        ),
    };
    let source = workspace.note_policy_source(policy).with_context(|| {
        format!(
            "create note in folder {:?} names no Markdown source of workspace {:?}",
            policy.folder, workspace.name
        )
    })?;
    let folder = policy.folder.trim_end_matches('/');
    let note_folder = if folder == "." || folder.is_empty() {
        None
    } else {
        let folder = folder.strip_prefix("./").unwrap_or(folder);
        let relative = PathBuf::from(folder);
        ensure!(
            relative
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
            "create note folder {:?} must stay within the Markdown source",
            policy.folder
        );
        Some(relative)
    };
    Ok((source.name.clone(), note_folder))
}

fn view_policy(workspace: &enzyme_spec::Workspace) -> WorkspacePolicy {
    WorkspacePolicy {
        excluded_folders: workspace.exclusions.clone(),
        excluded_tags: workspace.excluded_tags.clone(),
        entities: workspace
            .readings
            .iter()
            .filter(|reading| !reading.pattern)
            .map(|reading| {
                let (profile, expandable) = reading_options(reading);
                if profile.is_none() && !expandable {
                    WorkspaceEntity::simple(reading.entity.clone())
                } else {
                    WorkspaceEntity::with_options(
                        reading.entity.clone(),
                        WorkspaceEntityOptions {
                            profile,
                            expandable,
                            children: Vec::new(),
                        },
                    )
                }
            })
            .collect(),
        excluded_entities: workspace
            .excluded_links
            .iter()
            .map(|link| enzyme_spec::entity_selector("link", link))
            .collect(),
    }
}

fn reading_options(reading: &Reading) -> (Option<String>, bool) {
    let profile = (reading.profile != "auto").then(|| reading.profile.clone());
    (profile, reading.include_linked_pages)
}

fn host_binding(host: &HostSource) -> Result<WorkspaceBinding> {
    let label = || format!("source {} {:?}", host.kind, host.name);
    let allowed: &[&str] = match host.kind.as_str() {
        SOURCE_CAPTURES => &[FIELD_PATH],
        SOURCE_GOOGLE_MAIL => &[FIELD_ACCOUNT, FIELD_QUERY, FIELD_BACKFILL_DAYS],
        SOURCE_GOOGLE_CALENDAR => &[FIELD_ACCOUNT, FIELD_LOOKBACK_DAYS, FIELD_LOOKAHEAD_DAYS],
        SOURCE_GOOGLE_MEET => &[FIELD_ACCOUNT],
        SOURCE_GRANOLA => &[FIELD_ACCOUNT, FIELD_TIME_RANGE, FIELD_WORKSPACE_ONLY],
        other => bail!(
            "{}: unknown source kind {other:?}; Margins Workspaces declare markdown, {SOURCE_CAPTURES}, {SOURCE_GOOGLE_MAIL}, {SOURCE_GOOGLE_CALENDAR}, {SOURCE_GOOGLE_MEET}, or {SOURCE_GRANOLA} sources",
            label()
        ),
    };
    for field in &host.fields {
        ensure!(
            allowed.contains(&field.key.as_str()),
            "{}: unknown field {:?}; expected {}",
            label(),
            field.key,
            allowed.join(", ")
        );
    }
    let text = |key: &str| -> Result<Option<String>> {
        match host.field(key) {
            None => Ok(None),
            Some(HostValue::Text(value)) => Ok(Some(value.clone())),
            Some(_) => bail!("{}: {key} must be quoted text", label()),
        }
    };
    let days = |key: &str| -> Result<Option<u32>> {
        match host.field(key) {
            None => Ok(None),
            Some(HostValue::Integer(value)) => u32::try_from(*value)
                .map(Some)
                .with_context(|| format!("{}: {key} is too large", label())),
            Some(_) => bail!("{}: {key} must be a whole number of days", label()),
        }
    };
    let flag = |key: &str| -> Result<Option<bool>> {
        match host.field(key) {
            None => Ok(None),
            Some(HostValue::Bool(value)) => Ok(Some(*value)),
            Some(_) => bail!("{}: {key} must be true or false", label()),
        }
    };
    let account = || -> Result<String> {
        text(FIELD_ACCOUNT)?.with_context(|| format!("{} requires account \"…\"", label()))
    };
    Ok(match host.kind.as_str() {
        SOURCE_CAPTURES => WorkspaceBinding::Captures {
            path: expand_path(
                &text(FIELD_PATH)?.with_context(|| format!("{} requires path \"…\"", label()))?,
            ),
        },
        SOURCE_GOOGLE_MAIL => {
            let defaults = GmailCollectionSelector::default_declaration();
            WorkspaceBinding::Gmail {
                account: account()?,
                gmail: GmailCollectionSelector {
                    query: text(FIELD_QUERY)?.unwrap_or(defaults.query),
                    backfill_days: days(FIELD_BACKFILL_DAYS)?.unwrap_or(defaults.backfill_days),
                },
            }
        }
        SOURCE_GOOGLE_CALENDAR => {
            let defaults = CalendarCollectionSelector::default_declaration();
            WorkspaceBinding::GoogleCalendar {
                account: account()?,
                calendar: CalendarCollectionSelector {
                    lookback_days: days(FIELD_LOOKBACK_DAYS)?.unwrap_or(defaults.lookback_days),
                    lookahead_days: days(FIELD_LOOKAHEAD_DAYS)?
                        .unwrap_or(defaults.lookahead_days),
                },
            }
        }
        SOURCE_GOOGLE_MEET => WorkspaceBinding::GoogleMeet { account: account()? },
        SOURCE_GRANOLA => {
            let defaults = GranolaCollectionSelector::default_declaration();
            let time_range = match text(FIELD_TIME_RANGE)?.as_deref() {
                None => defaults.time_range,
                Some(value) if value == GranolaTimeRange::Last30Days.as_provider_value() => {
                    GranolaTimeRange::Last30Days
                }
                Some(other) => bail!(
                    "{}: time range {other:?} is not supported; use \"{}\"",
                    label(),
                    GranolaTimeRange::Last30Days.as_provider_value()
                ),
            };
            WorkspaceBinding::Granola {
                account: account()?,
                collection: GranolaCollectionSelector {
                    time_range,
                    workspace_only: flag(FIELD_WORKSPACE_ONLY)?.unwrap_or(defaults.workspace_only),
                },
            }
        }
        _ => unreachable!("kind checked above"),
    })
}

// ---------------------------------------------------------------------------
// View -> program
// ---------------------------------------------------------------------------

/// The program source for one binding. Defaults are omitted.
pub fn source_from_binding(name: &str, binding: &WorkspaceBinding) -> Source {
    let host = |kind: &str, fields: Vec<HostField>| {
        Source::Host(HostSource {
            kind: kind.to_string(),
            name: name.to_string(),
            fields,
        })
    };
    let text = |key: &str, value: &str| HostField {
        key: key.to_string(),
        value: HostValue::Text(value.to_string()),
    };
    let integer = |key: &str, value: u32| HostField {
        key: key.to_string(),
        value: HostValue::Integer(u64::from(value)),
    };
    match binding {
        WorkspaceBinding::NativeMarkdown { path, .. } => Source::Markdown(MarkdownSource {
            name: name.to_string(),
            path: path_text(path),
        }),
        WorkspaceBinding::Captures { path } => {
            host(SOURCE_CAPTURES, vec![text(FIELD_PATH, &path_text(path))])
        }
        WorkspaceBinding::Gmail { account, gmail } => {
            let defaults = GmailCollectionSelector::default_declaration();
            let mut fields = vec![text(FIELD_ACCOUNT, account)];
            if gmail.query != defaults.query {
                fields.push(text(FIELD_QUERY, &gmail.query));
            }
            if gmail.backfill_days != defaults.backfill_days {
                fields.push(integer(FIELD_BACKFILL_DAYS, gmail.backfill_days));
            }
            host(SOURCE_GOOGLE_MAIL, fields)
        }
        WorkspaceBinding::GoogleCalendar { account, calendar } => {
            let defaults = CalendarCollectionSelector::default_declaration();
            let mut fields = vec![text(FIELD_ACCOUNT, account)];
            if calendar.lookback_days != defaults.lookback_days {
                fields.push(integer(FIELD_LOOKBACK_DAYS, calendar.lookback_days));
            }
            if calendar.lookahead_days != defaults.lookahead_days {
                fields.push(integer(FIELD_LOOKAHEAD_DAYS, calendar.lookahead_days));
            }
            host(SOURCE_GOOGLE_CALENDAR, fields)
        }
        WorkspaceBinding::GoogleMeet { account } => {
            host(SOURCE_GOOGLE_MEET, vec![text(FIELD_ACCOUNT, account)])
        }
        WorkspaceBinding::Granola {
            account,
            collection,
        } => {
            let defaults = GranolaCollectionSelector::default_declaration();
            let mut fields = vec![text(FIELD_ACCOUNT, account)];
            if collection.time_range != defaults.time_range {
                fields.push(text(
                    FIELD_TIME_RANGE,
                    collection.time_range.as_provider_value(),
                ));
            }
            if collection.workspace_only != defaults.workspace_only {
                fields.push(HostField {
                    key: FIELD_WORKSPACE_ONLY.to_string(),
                    value: HostValue::Bool(collection.workspace_only),
                });
            }
            host(SOURCE_GRANOLA, fields)
        }
    }
}

/// How unqualified folder readings in the desired view are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderQualification {
    /// The view was derived from the base program; its folder references use
    /// the base program's qualification.
    Program,
    /// A retired `config.toml`: an unqualified folder ref meant the Home source.
    Legacy,
}

/// A minimal program for one Workspace id, before any source is declared.
pub fn empty_program(id: &str) -> Program {
    Program {
        workspaces: vec![enzyme_spec::Workspace {
            name: id.to_string(),
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// Edit `base` so its view equals `desired`, preserving everything the view
/// cannot express. Unchanged sources and readings keep their exact AST.
pub fn reconcile(
    base: &Program,
    desired: &WorkspaceConfig,
    qualification: FolderQualification,
) -> Result<Program> {
    ensure!(
        base.workspaces.len() == 1,
        "a Workspace program must contain exactly one workspace block"
    );
    let mut program = base.clone();
    let workspace = &mut program.workspaces[0];
    ensure!(
        workspace.name == desired.id,
        "desired Workspace id {:?} does not match program workspace {:?}",
        desired.id,
        workspace.name
    );
    let base_markdown: Vec<String> = workspace
        .markdown_sources()
        .map(|source| source.name.clone())
        .collect();

    // Sources: keep declaration order; replace changed ones; append new ones.
    let mut sources = Vec::new();
    let mut removed = BTreeSet::new();
    for source in &workspace.sources {
        let name = source.name().to_string();
        match desired.bindings.get(&name) {
            None => {
                removed.insert(name.to_ascii_lowercase());
            }
            Some(binding) => {
                if source_matches_binding(source, binding) {
                    sources.push(source.clone());
                } else {
                    sources.push(source_from_binding(&name, binding));
                }
            }
        }
    }
    for (name, binding) in &desired.bindings {
        if !workspace
            .sources
            .iter()
            .any(|source| source.name() == name.as_str())
        {
            sources.push(source_from_binding(name, binding));
        }
    }
    workspace.sources = sources;
    let markdown: Vec<String> = workspace
        .markdown_sources()
        .map(|source| source.name.clone())
        .collect();

    // Home: the one create-note policy, keeping its conditions and guidance.
    let homes: Vec<(&String, &Option<PathBuf>)> = desired
        .bindings
        .iter()
        .filter_map(|(name, binding)| match binding {
            WorkspaceBinding::NativeMarkdown {
                role: SourceRole::Home,
                note_folder,
                ..
            } => Some((name, note_folder)),
            _ => None,
        })
        .collect();
    let [(home, note_folder)] = homes.as_slice() else {
        bail!("workspace must declare exactly one Home Markdown source");
    };
    let home = (*home).clone();
    let mut policy = workspace
        .note_policies
        .first()
        .cloned()
        .unwrap_or(NotePolicy {
            folder: ".".to_string(),
            source: None,
            conditions: Vec::new(),
            guidance: Vec::new(),
            root: None,
        });
    policy.folder = note_folder
        .as_ref()
        .map(|folder| {
            folder
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|| ".".to_string());
    policy.source = if markdown.len() > 1 {
        Some(home.clone())
    } else if base_markdown.len() > 1 {
        // Several roots became one: the qualifier is no longer needed.
        None
    } else {
        // Keep an explicit qualifier the author wrote for a lone root.
        policy
            .source
            .filter(|source| source.eq_ignore_ascii_case(&home))
    };
    policy.root = None;
    workspace.note_policies = vec![policy];

    // Readings: one per desired entity, reusing the existing AST when present.
    let mut readings = Vec::new();
    let mut seen = BTreeSet::new();
    for configured in &desired.policy.entities {
        for (entity_ref, options) in configured.entries() {
            let entity = normalize_entity_ref(entity_ref)?;
            if references_removed_source(&entity, &base_markdown, &removed) {
                continue;
            }
            let existing = workspace
                .readings
                .iter()
                .find(|reading| !reading.pattern && reading.entity.eq_ignore_ascii_case(&entity))
                .cloned();
            let qualified = qualify_folder(&entity, qualification, &base_markdown, &markdown, &home);
            if !seen.insert(qualified.to_lowercase()) {
                continue;
            }
            let mut reading = existing.unwrap_or_else(|| Reading {
                entity: qualified.clone(),
                profile: "auto".to_string(),
                definition: None,
                learning: Learning::default(),
                include_linked_pages: false,
                include_who_links: false,
                pattern: false,
            });
            reading.entity = qualified;
            let desired_profile = options.and_then(|options| options.profile.clone());
            let desired_expandable = options.is_some_and(|options| options.expandable)
                && enzyme_spec::split_entity(&reading.entity).0 == "folder";
            let (current_profile, _) = reading_options(&reading);
            if current_profile != desired_profile {
                reading.profile = desired_profile.unwrap_or_else(|| "auto".to_string());
                reading.definition = None;
            }
            reading.include_linked_pages = desired_expandable;
            readings.push(reading);
        }
    }
    enzyme_spec::retain_patterns(&workspace.readings, &mut readings);
    workspace.readings = readings;

    // Exclusions. Enzyme's implicit exclusions are never written.
    let mut folders = Vec::new();
    let mut tags = Vec::new();
    let mut links = Vec::new();
    for folder in &desired.policy.excluded_folders {
        push_unique(&mut folders, folder.trim().trim_matches('/'));
    }
    for tag in &desired.policy.excluded_tags {
        push_unique(&mut tags, tag.trim().trim_start_matches('#'));
    }
    for entity in &desired.policy.excluded_entities {
        let (kind, name) = enzyme_spec::split_entity(entity.trim());
        match kind {
            "folder" => push_unique(&mut folders, name.trim_matches('/')),
            "tag" => push_unique(&mut tags, name),
            "link" => push_unique(&mut links, name),
            other => bail!("excluded entity {entity:?} has unsupported kind {other:?}; leave out folders, tags, or links"),
        }
    }
    folders.retain(|folder| !enzyme_spec::is_implicit_exclusion(folder));
    workspace.exclusions = folders;
    workspace.excluded_tags = tags;
    workspace.excluded_links = links;
    Ok(program)
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !list.iter().any(|existing| existing.eq_ignore_ascii_case(value)) {
        list.push(value.to_string());
    }
}

fn source_matches_binding(source: &Source, binding: &WorkspaceBinding) -> bool {
    match (source, binding) {
        (Source::Markdown(markdown), WorkspaceBinding::NativeMarkdown { path, .. }) => {
            expand_path(&markdown.path) == *path
        }
        (Source::Host(host), binding) => host_binding(host).is_ok_and(|current| current == *binding),
        _ => false,
    }
}

/// Canonical reading entity for a view entity ref (`folder:x`, `#tag`,
/// `[[link]]`, `tag:x`, `source:mail`, …).
pub fn normalize_entity_ref(entity_ref: &str) -> Result<String> {
    let trimmed = entity_ref.trim();
    let (kind, name) = enzyme_spec::split_entity(trimmed);
    let kind = kind.to_ascii_lowercase();
    ensure!(
        !name.trim().is_empty(),
        "entity {entity_ref:?} needs a name"
    );
    match kind.as_str() {
        "folder" | "tag" | "link" | "log" | "source" | "channel" | "collection" | "thread" => {}
        other => bail!(
            "entity {entity_ref:?} has unsupported kind {other:?}; use folder:, #tag, [[link]], log:, or source:"
        ),
    }
    let name = if kind == "folder" {
        let name = name.trim().trim_matches('/');
        if name.is_empty() { "." } else { name }
    } else {
        name.trim()
    };
    Ok(enzyme_spec::entity_selector(&kind, name))
}

fn references_removed_source(entity: &str, base_markdown: &[String], removed: &BTreeSet<String>) -> bool {
    let (kind, name) = enzyme_spec::split_entity(entity);
    match kind {
        "source" => removed.contains(&name.to_ascii_lowercase()),
        "thread" => name
            .split_once('/')
            .is_some_and(|(source, _)| removed.contains(&source.to_ascii_lowercase())),
        "folder" if base_markdown.len() > 1 => {
            let first = name.split('/').next().unwrap_or(name).to_ascii_lowercase();
            removed.contains(&first)
        }
        _ => false,
    }
}

fn qualify_folder(
    entity: &str,
    qualification: FolderQualification,
    base_markdown: &[String],
    markdown: &[String],
    home: &str,
) -> String {
    let (kind, name) = enzyme_spec::split_entity(entity);
    if kind != "folder" {
        return entity.to_string();
    }
    let first = name.split('/').next().unwrap_or(name);
    let names_source = |names: &[String]| names.iter().any(|source| source.eq_ignore_ascii_case(first));
    let prefixed = |path: &str| {
        if path == "." {
            enzyme_spec::entity_selector("folder", home)
        } else {
            enzyme_spec::entity_selector("folder", &format!("{home}/{path}"))
        }
    };
    let base_qualified = qualification == FolderQualification::Program && base_markdown.len() > 1;
    if markdown.len() > 1 {
        if base_qualified || (qualification == FolderQualification::Legacy && names_source(markdown)) {
            entity.to_string()
        } else {
            prefixed(name)
        }
    } else if base_qualified && names_source(markdown) {
        // Several roots became one: drop the remaining root's prefix.
        let rest = name
            .split_once('/')
            .map(|(_, rest)| rest)
            .unwrap_or(".");
        enzyme_spec::entity_selector("folder", rest)
    } else {
        entity.to_string()
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a program through `enzyme_spec::resolve` after replacing every
/// Margins host source with a placeholder of the native kind it lowers to.
/// This checks readings, profiles, folder qualification, and exclusions with
/// the same resolver the engine uses. `profiles` is the optional shared
/// `configs/profiles.enzyme` program.
pub fn validate_language(program: &WorkspaceProgram, profiles: Option<&Program>) -> Result<()> {
    let mut lowered = program.program().clone();
    lowered.lower_host_sources(|_, host| {
        Ok(Some(Source::Sqlite(SqliteSource {
            name: host.name.clone(),
            db: "/margins/ledger.db".to_string(),
            query: "select 1".to_string(),
            id: vec!["id".to_string()],
            document_ref: None,
            who: SqliteWho::Columns {
                columns: vec!["who".to_string()],
            },
            when: "when".to_string(),
            what: vec!["what".to_string()],
            where_columns: Vec::new(),
            weight: None,
            timestamp_unit: "milliseconds".to_string(),
            timestamp_epoch: None,
            filter: None,
        })))
    })?;
    let mut programs = vec![lowered];
    if let Some(profiles) = profiles {
        programs.push(profiles.clone());
    }
    enzyme_spec::resolve(programs, &user_home())?;
    Ok(())
}

/// A unified diff between two program texts.
pub fn unified_diff(before: &str, after: &str, path: &str) -> String {
    similar::TextDiff::from_lines(before, after)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_KINDS: &str = r#"
profile clients {
  seek "what each client needs next"
  notice {
    "commitments"
  }
}

workspace "practice" {
  source markdown "notes" { path "/abs/notes" }
  source markdown "library" { path "/abs/library" }
  source margins-captures "captures" { path "/abs/state/captures" }
  source google-mail "mail" {
    account "me@example.com"
    query "label:clients"
    backfill days 90
  }
  source google-calendar "calendar" {
    account "me@example.com"
    lookback days 30
    lookahead days 14
  }
  source google-meet "meet" { account "me@example.com" }
  source granola "granola" {
    account "me@example.com"
    time range "last_30_days"
    workspace only true
  }

  remember in folder "inbox" in source "notes" when {
    "A meeting ended with a decision."
  } create note {
    "Start with the decision."
  }

  leave out folders { "archive" }
  leave out tags { "private" }
  leave out links { "Spam Sender" }
  learn questions from folder "notes/people" including linked pages about relationships
  learn questions from source "mail" about clients {
    sample by time
  }
  learn questions from tags matching "proj-*"
  when asked {
    "Use grep for exact names."
  }
}
"#;

    fn parse(text: &str) -> WorkspaceProgram {
        WorkspaceProgram::parse(text).unwrap()
    }

    fn view(text: &str) -> Result<WorkspaceConfig> {
        derive_view(&parse(text), None, RetentionPolicy::default())
    }

    #[test]
    fn every_source_kind_derives_its_binding() {
        let config = view(ALL_KINDS).unwrap();
        assert_eq!(config.id, "practice");
        assert_eq!(
            config.bindings["notes"],
            WorkspaceBinding::NativeMarkdown {
                path: "/abs/notes".into(),
                role: SourceRole::Home,
                note_folder: Some("inbox".into()),
            }
        );
        assert_eq!(
            config.bindings["library"],
            WorkspaceBinding::NativeMarkdown {
                path: "/abs/library".into(),
                role: SourceRole::Reference,
                note_folder: None,
            }
        );
        assert_eq!(
            config.bindings["captures"],
            WorkspaceBinding::Captures {
                path: "/abs/state/captures".into()
            }
        );
        assert_eq!(
            config.bindings["mail"],
            WorkspaceBinding::Gmail {
                account: "me@example.com".into(),
                gmail: GmailCollectionSelector {
                    query: "label:clients".into(),
                    backfill_days: 90,
                },
            }
        );
        assert_eq!(
            config.bindings["calendar"],
            WorkspaceBinding::GoogleCalendar {
                account: "me@example.com".into(),
                calendar: CalendarCollectionSelector {
                    lookback_days: 30,
                    lookahead_days: 14,
                },
            }
        );
        assert_eq!(
            config.bindings["meet"],
            WorkspaceBinding::GoogleMeet {
                account: "me@example.com".into()
            }
        );
        assert_eq!(
            config.bindings["granola"],
            WorkspaceBinding::Granola {
                account: "me@example.com".into(),
                collection: GranolaCollectionSelector {
                    time_range: GranolaTimeRange::Last30Days,
                    workspace_only: true,
                },
            }
        );
        assert_eq!(config.policy.excluded_folders, vec!["archive"]);
        assert_eq!(config.policy.excluded_tags, vec!["private"]);
        assert_eq!(config.policy.excluded_entities, vec!["[[Spam Sender]]"]);
        // Name patterns are not view entities.
        assert_eq!(
            config.policy.entities,
            vec![
                WorkspaceEntity::with_options(
                    "folder:notes/people",
                    WorkspaceEntityOptions {
                        profile: Some("relationships".into()),
                        expandable: true,
                        children: Vec::new(),
                    },
                ),
                WorkspaceEntity::with_options(
                    "source:mail",
                    WorkspaceEntityOptions {
                        profile: Some("clients".into()),
                        expandable: false,
                        children: Vec::new(),
                    },
                ),
            ]
        );
        validate_language(&parse(ALL_KINDS), None).unwrap();
    }

    #[test]
    fn defaults_are_omitted_and_bindings_round_trip_through_sources() {
        let config = view(ALL_KINDS).unwrap();
        for (name, binding) in &config.bindings {
            let source = source_from_binding(name, binding);
            assert!(source_matches_binding(&source, binding), "{name}");
        }
        let gmail = source_from_binding(
            "mail",
            &WorkspaceBinding::Gmail {
                account: "me@example.com".into(),
                gmail: GmailCollectionSelector::default_declaration(),
            },
        );
        let Source::Host(host) = gmail else { panic!() };
        assert_eq!(host.fields.len(), 1);
        let granola = source_from_binding(
            "granola",
            &WorkspaceBinding::Granola {
                account: "me@example.com".into(),
                collection: GranolaCollectionSelector::default_declaration(),
            },
        );
        let Source::Host(host) = granola else { panic!() };
        assert_eq!(host.fields.len(), 1);
    }

    #[test]
    fn render_parse_round_trip_is_stable() {
        let first = WorkspaceProgram::from_program(parse(ALL_KINDS).program().clone()).unwrap();
        let second = WorkspaceProgram::from_program(first.program().clone()).unwrap();
        assert_eq!(first.text(), second.text());
        assert_eq!(view(first.text()).unwrap(), view(ALL_KINDS).unwrap());
        // Reconciling a program toward its own view changes nothing.
        let same = reconcile(
            first.program(),
            &view(first.text()).unwrap(),
            FolderQualification::Program,
        )
        .unwrap();
        assert_eq!(WorkspaceProgram::from_program(same).unwrap().text(), first.text());
    }

    #[test]
    fn home_requires_exactly_one_create_note_policy_inside_its_source() {
        let without = "workspace \"w\" {\n  source markdown \"notes\" { path \"/abs/notes\" }\n}\n";
        assert!(view(without)
            .unwrap_err()
            .to_string()
            .contains("must declare where Margins writes notes"));

        let two = "workspace \"w\" {\n  source markdown \"notes\" { path \"/abs/notes\" }\n  remember in folder \"a\" create note\n  remember in folder \"b\" create note\n}\n";
        assert!(view(two)
            .unwrap_err()
            .to_string()
            .contains("exactly one folder"));

        for folder in ["../outside", "/abs/elsewhere", "~/notes", "inbox/../../x"] {
            let text = format!(
                "workspace \"w\" {{\n  source markdown \"notes\" {{ path \"/abs/notes\" }}\n  remember in folder {folder:?} create note\n}}\n"
            );
            assert!(WorkspaceProgram::parse(&text).is_err(), "{folder}");
        }

        let ambiguous = "workspace \"w\" {\n  source markdown \"a\" { path \"/abs/a\" }\n  source markdown \"b\" { path \"/abs/b\" }\n  remember in folder \"inbox\" create note\n}\n";
        assert!(WorkspaceProgram::parse(ambiguous).is_err());

        let root = "workspace \"w\" {\n  source markdown \"notes\" { path \"/abs/notes\" }\n  remember in folder \".\" create note\n}\n";
        let config = view(root).unwrap();
        assert_eq!(
            config.bindings["notes"],
            WorkspaceBinding::NativeMarkdown {
                path: "/abs/notes".into(),
                role: SourceRole::Home,
                note_folder: None,
            }
        );
    }

    #[test]
    fn host_sources_are_checked_strictly() {
        let unknown_field = "workspace \"w\" {\n  source markdown \"notes\" { path \"/abs/notes\" }\n  source google-mail \"mail\" { account \"me@example.com\" folder \"x\" }\n  remember in folder \".\" create note\n}\n";
        assert!(view(unknown_field).unwrap_err().to_string().contains("unknown field \"folder\""));
        let unknown_kind = "workspace \"w\" {\n  source markdown \"notes\" { path \"/abs/notes\" }\n  source slack \"chat\" { account \"me@example.com\" }\n  remember in folder \".\" create note\n}\n";
        assert!(view(unknown_kind).unwrap_err().to_string().contains("unknown source kind"));
        let no_account = "workspace \"w\" {\n  source markdown \"notes\" { path \"/abs/notes\" }\n  source google-meet \"meet\" {}\n  remember in folder \".\" create note\n}\n";
        assert!(view(no_account).unwrap_err().to_string().contains("requires account"));
        let wrong_type = "workspace \"w\" {\n  source markdown \"notes\" { path \"/abs/notes\" }\n  source google-mail \"mail\" { account \"me@example.com\" backfill days \"many\" }\n  remember in folder \".\" create note\n}\n";
        assert!(view(wrong_type).unwrap_err().to_string().contains("whole number"));
    }

    const SINGLE: &str = r#"profile clients {
  seek "what each client needs next"
  notice {
    "commitments"
  }
}

workspace "practice" {
  source markdown "home" { path "/abs/notes" }
  source margins-captures "captures" { path "/abs/state/captures" }

  learn questions from folder "people" about clients {
    sample by time
  }
  learn questions from tags matching "proj-*"
  leave out folders { "archive" }
  when asked {
    "Use grep for exact names."
  }
  remember in folder "inbox" when {
    "A meeting ended."
  } create note
}
"#;

    fn mutated(base: &WorkspaceProgram, edit: impl FnOnce(&mut WorkspaceConfig)) -> WorkspaceProgram {
        let mut desired = view(base.text()).unwrap();
        edit(&mut desired);
        WorkspaceProgram::from_program(
            reconcile(base.program(), &desired, FolderQualification::Program).unwrap(),
        )
        .unwrap()
    }

    fn assert_language_kept(program: &WorkspaceProgram) {
        let text = program.text();
        assert!(text.contains("profile clients {"), "{text}");
        assert!(text.contains("sample by time"), "{text}");
        assert!(text.contains("about clients"), "{text}");
        assert!(text.contains("learn questions from tags matching \"proj-*\""), "{text}");
        assert!(text.contains("\"Use grep for exact names.\""), "{text}");
        assert!(text.contains("\"A meeting ended.\""), "{text}");
        validate_language(program, None).unwrap();
    }

    #[test]
    fn mutations_preserve_language_the_view_cannot_express() {
        let base = parse(SINGLE);
        let with_mail = mutated(&base, |config| {
            config.bindings.insert(
                "mail".into(),
                WorkspaceBinding::Gmail {
                    account: "me@example.com".into(),
                    gmail: GmailCollectionSelector::default_declaration(),
                },
            );
        });
        assert_language_kept(&with_mail);
        assert!(with_mail.text().contains("source google-mail \"mail\" { account \"me@example.com\" }"));

        // Folder readings become root-qualified with a second Markdown source,
        // and the create-note policy names its source.
        let with_library = mutated(&with_mail, |config| {
            config.bindings.insert(
                "library".into(),
                WorkspaceBinding::NativeMarkdown {
                    path: "/abs/library".into(),
                    role: SourceRole::Reference,
                    note_folder: None,
                },
            );
        });
        assert_language_kept(&with_library);
        assert!(with_library.text().contains("learn questions from folder \"home/people\""));
        assert!(with_library.text().contains("remember in folder \"inbox\" in source \"home\""));

        // Back to one root: the prefix and the source qualifier go away.
        let without_library = mutated(&with_library, |config| {
            config.bindings.remove("library");
        });
        assert_language_kept(&without_library);
        assert!(without_library.text().contains("learn questions from folder \"people\""));
        assert!(!without_library.text().contains("in source \"home\""));

        // Removing a source drops readings of it; changing a profile drops an
        // inline definition but keeps the learning settings.
        let mut reading_mail = mutated(&without_library, |config| {
            config.policy.entities.push(WorkspaceEntity::simple("source:mail"));
        });
        assert!(reading_mail.text().contains("learn questions from source \"mail\""));
        reading_mail = mutated(&reading_mail, |config| {
            config.bindings.remove("mail");
        });
        assert!(!reading_mail.text().contains("source \"mail\""), "{}", reading_mail.text());
        assert_language_kept(&reading_mail);

        let reprofiled = mutated(&reading_mail, |config| {
            config.policy.entities[0] = WorkspaceEntity::with_options(
                "folder:people",
                WorkspaceEntityOptions {
                    profile: Some("decisions".into()),
                    expandable: true,
                    children: Vec::new(),
                },
            );
        });
        let text = reprofiled.text();
        assert!(text.contains("including linked pages\n    about decisions"), "{text}");
        assert!(text.contains("sample by time"), "{text}");
    }

    #[test]
    fn exclusions_split_by_kind_and_skip_implicit_folders() {
        let base = parse(SINGLE);
        let program = mutated(&base, |config| {
            config.policy.excluded_folders = vec![".git".into(), "node_modules".into(), "archive".into()];
            config.policy.excluded_entities =
                vec!["folder:old".into(), "#draft".into(), "[[Noise]]".into(), "Plain".into()];
        });
        let workspace = program.workspace();
        assert_eq!(workspace.exclusions, vec!["archive", "old"]);
        assert_eq!(workspace.excluded_tags, vec!["draft"]);
        assert_eq!(workspace.excluded_links, vec!["Noise", "Plain"]);
    }

    #[test]
    fn unsupported_entity_kinds_are_refused() {
        assert!(normalize_entity_ref("person:ada").is_err());
        assert_eq!(normalize_entity_ref("tag:Craft").unwrap(), "#Craft");
        assert_eq!(normalize_entity_ref("[[Ada]]").unwrap(), "[[Ada]]");
        assert_eq!(normalize_entity_ref("folder:/people/").unwrap(), "folder:people");
    }
}
