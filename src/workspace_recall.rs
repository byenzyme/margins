use anyhow::{Context, Result};
use margins_workflows::integrations::{EvidenceHandle, GoogleCalendarScope};
use margins_workflows::workspace::{
    calendar_collection_namespace, gmail_collection_namespace, granola_collection_namespace,
    meet_collection_namespace, native_markdown_collection_namespace, ResolvedWorkspace, SourceKind,
    SourceRole, WorkspaceBinding, WorkspaceEntity, WorkspaceEntityOptions,
};
use recall_engine::config::NotesRootConfig;
use recall_engine::document::{DiscoveredFile, FileDiscovery};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(crate) struct CatalogEntry {
    pub source: String,
    pub kind: SourceKind,
    pub evidence: EvidenceHandle,
}

#[derive(Debug)]
pub(crate) struct WorkspaceCorpus {
    _runtime: tempfile::TempDir,
    pub virtual_home: PathBuf,
    pub engine_config: PathBuf,
    pub catalog: BTreeMap<String, CatalogEntry>,
    pub filesystem_documents: BTreeMap<String, PathBuf>,
    pub sqlite_sources: BTreeMap<String, PathBuf>,
    pub entity_selection_debug: EntitySelectionDebug,
    pub excluded_link_entities: BTreeSet<String>,
    pub selected_link_entities: BTreeSet<String>,
    pub selected_entity_names: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EntitySelectionDebug {
    pub selected: Vec<(String, Option<u64>)>,
    pub suppressed_count: usize,
    pub correspondents_considered: usize,
}

#[derive(Clone, Serialize)]
struct EngineConfig {
    defaults: EngineDefaults,
    workspaces: BTreeMap<String, EngineWorkspace>,
}

#[derive(Clone, Serialize)]
struct EngineDefaults {
    total_limit: usize,
}

#[derive(Clone, Serialize)]
struct EngineWorkspace {
    excluded_folders: Vec<String>,
    excluded_tags: Vec<String>,
    excluded_links: Vec<String>,
    entities: Vec<WorkspaceEntity>,
    sources: BTreeMap<String, EngineSource>,
}

#[derive(Clone, Serialize)]
#[serde(untagged)]
enum EngineSource {
    Notes {
        path: PathBuf,
        document_ref_prefix: String,
        exclusions: Vec<String>,
        writable: bool,
    },
    Sqlite {
        db: PathBuf,
        query: String,
        roles: EngineRoles,
        timestamp: EngineTimestamp,
    },
}

#[derive(Clone, Serialize)]
struct EngineRoles {
    id: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_ref: Option<String>,
    who: EngineWhoRole,
    when: String,
    what: Vec<String>,
}

#[derive(Clone, Serialize)]
struct EngineWhoRole {
    format: &'static str,
    column: String,
}

pub(crate) const ENGINE_ENTITY_LIMIT: usize = 30;
const EMPTY_LEDGER_SELECTION_SENTINEL: &str = "[[margins:empty-ledger-selection]]";
const MANAGED_PROJECTION_TAG: &str = "margins-managed-projection";

#[derive(Clone, Serialize)]
struct EngineTimestamp {
    unit: &'static str,
}

pub(crate) fn prepare(
    workspace: &ResolvedWorkspace,
    validate_sources: bool,
) -> Result<WorkspaceCorpus> {
    let runtime = tempfile::tempdir().context("creating ephemeral recall workspace")?;
    let virtual_workspaces = runtime.path().join("workspaces");
    std::fs::create_dir_all(&virtual_workspaces)?;
    let virtual_home = virtual_workspaces.join(&workspace.config.id);
    std::fs::create_dir_all(&virtual_home)?;

    let mut sources = BTreeMap::new();
    let mut catalog = BTreeMap::new();
    let mut filesystem_documents = BTreeMap::new();
    let mut sqlite_sources = BTreeMap::new();
    let notes_roots = workspace
        .config
        .bindings
        .iter()
        .filter_map(|(name, binding)| match binding {
            WorkspaceBinding::NativeMarkdown { path, role, .. } => Some(
                native_markdown_collection_namespace(path).map(|document_ref_prefix| {
                    NotesRootConfig {
                        name: name.clone(),
                        path: path.clone(),
                        document_ref_prefix: Some(document_ref_prefix),
                        exclusions: workspace.config.policy.excluded_folders.clone(),
                        writable: *role == SourceRole::Home,
                    }
                }),
            ),
            _ => None,
        })
        .collect::<Result<Vec<_>>>()?;
    let mut markdown_folder_entities = MarkdownFolderEntityIndex::default();
    discover_markdown_roots(
        &notes_roots,
        &mut catalog,
        &mut filesystem_documents,
        &mut markdown_folder_entities,
    )?;
    for root in &notes_roots {
        sources.insert(
            root.name.clone(),
            EngineSource::Notes {
                path: root.path.clone(),
                document_ref_prefix: root
                    .document_ref_prefix
                    .clone()
                    .expect("Margins always assigns native Markdown document identity"),
                exclusions: root.exclusions.clone(),
                writable: root.writable,
            },
        );
    }

    for binding in workspace.config.bindings.values() {
        match binding {
            WorkspaceBinding::Gmail { account, .. } => {
                if !workspace.ledger_path().is_file() {
                    continue;
                }
                let source_name = gmail_collection_namespace(account)?;
                sqlite_sources.insert(source_name.clone(), workspace.ledger_path());
                sources.insert(
                    source_name.clone(),
                    mail_thread_sqlite_source(
                        &workspace.ledger_path(),
                        "email",
                        account,
                        &source_name,
                    ),
                );
            }
            WorkspaceBinding::GoogleCalendar { account, calendar } => {
                if !workspace.ledger_path().is_file() {
                    continue;
                }
                let source_name = calendar_collection_namespace(account)?;
                let scope = GoogleCalendarScope::for_selector(calendar, chrono::Utc::now())?;
                sqlite_sources.insert(source_name.clone(), workspace.ledger_path());
                sources.insert(
                    source_name.clone(),
                    calendar_event_sqlite_source(
                        &workspace.ledger_path(),
                        "gcal",
                        account,
                        &source_name,
                        scope,
                    ),
                );
            }
            WorkspaceBinding::GoogleMeet { account } => {
                if !workspace.ledger_path().is_file() {
                    continue;
                }
                let source_name = meet_collection_namespace(account)?;
                sqlite_sources.insert(source_name.clone(), workspace.ledger_path());
                sources.insert(
                    source_name.clone(),
                    external_document_sqlite_source(
                        &workspace.ledger_path(),
                        "google_meet",
                        account,
                        &source_name,
                        None,
                    ),
                );
            }
            WorkspaceBinding::Granola { account, .. } => {
                if !workspace.ledger_path().is_file() {
                    continue;
                }
                let source_name = granola_collection_namespace(account)?;
                sqlite_sources.insert(source_name.clone(), workspace.ledger_path());
                sources.insert(
                    source_name.clone(),
                    external_document_sqlite_source(
                        &workspace.ledger_path(),
                        "granola",
                        account,
                        &source_name,
                        None,
                    ),
                );
            }
            _ => {}
        }
    }
    load_ledger_catalog(workspace, &mut catalog)?;

    if validate_sources {
        validate_sqlite_sources(&sources)?;
    }
    let mut entity_policy = catalyst_entity_policy(workspace, &markdown_folder_entities)?;
    let has_ledger_source = workspace.config.bindings.values().any(|binding| {
        matches!(
            binding,
            WorkspaceBinding::Gmail { .. }
                | WorkspaceBinding::GoogleCalendar { .. }
                | WorkspaceBinding::GoogleMeet { .. }
                | WorkspaceBinding::Granola { .. }
        )
    });
    if has_ledger_source && entity_policy.entities.is_empty() {
        // The engine treats an empty configured list as "no curated policy"
        // and falls back to raw occurrence coverage. A missing sentinel would
        // therefore turn a decode failure or all-noise mailbox into broad
        // catalyst generation.
        entity_policy
            .entities
            .push(WorkspaceEntity::simple(EMPTY_LEDGER_SELECTION_SENTINEL));
    }
    let engine_excluded_links = workspace
        .config
        .policy
        .excluded_entities
        .iter()
        .filter_map(|entity| link_entity_name(entity))
        .chain(entity_policy.excluded_links.iter().cloned())
        .collect::<BTreeSet<_>>();
    let selected_link_entities = entity_policy
        .entities
        .iter()
        .flat_map(WorkspaceEntity::entries)
        .filter_map(|(entity, _)| link_entity_name(entity))
        .collect();
    let selected_entity_names = entity_policy
        .entities
        .iter()
        .flat_map(WorkspaceEntity::entries)
        .filter(|(entity, _)| *entity != EMPTY_LEDGER_SELECTION_SENTINEL)
        .filter_map(|(entity, _)| engine_entity_name(entity))
        .collect();
    let config = EngineConfig {
        defaults: EngineDefaults {
            total_limit: ENGINE_ENTITY_LIMIT,
        },
        workspaces: BTreeMap::from([(
            workspace.config.id.clone(),
            EngineWorkspace {
                excluded_folders: workspace
                    .config
                    .policy
                    .excluded_folders
                    .iter()
                    .cloned()
                    .chain(
                        workspace
                            .config
                            .policy
                            .excluded_entities
                            .iter()
                            .filter_map(|entity| prefixed_entity_name(entity, "folder:")),
                    )
                    .collect(),
                excluded_tags: workspace
                    .config
                    .policy
                    .excluded_tags
                    .iter()
                    .cloned()
                    .chain(
                        workspace
                            .config
                            .policy
                            .excluded_entities
                            .iter()
                            .filter_map(|entity| prefixed_entity_name(entity, "#")),
                    )
                    .chain(std::iter::once(MANAGED_PROJECTION_TAG.to_string()))
                    .collect(),
                excluded_links: workspace
                    .config
                    .policy
                    .excluded_entities
                    .iter()
                    .filter_map(|entity| link_entity_name(entity))
                    .chain(entity_policy.excluded_links.iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                entities: entity_policy.entities.clone(),
                sources,
            },
        )]),
    };
    let engine_config = runtime.path().join("config.toml");
    std::fs::write(&engine_config, toml::to_string_pretty(&config)?)?;
    Ok(WorkspaceCorpus {
        _runtime: runtime,
        virtual_home,
        engine_config,
        catalog,
        filesystem_documents,
        sqlite_sources,
        entity_selection_debug: entity_policy.debug,
        excluded_link_entities: engine_excluded_links,
        selected_link_entities,
        selected_entity_names,
    })
}

/// Exhaust every configured SQLite query before recall-engine receives the
/// existing index. A bad ledger view/reference query must not let a refresh
/// prune documents before the source error is discovered.
fn validate_sqlite_sources(sources: &BTreeMap<String, EngineSource>) -> Result<()> {
    for (name, source) in sources {
        let EngineSource::Sqlite { db, query, .. } = source else {
            continue;
        };
        let connection =
            Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| {
                    format!("opening SQLite recall source {name} at {}", db.display())
                })?;
        let mut statement = connection
            .prepare(query)
            .with_context(|| format!("preflighting SQLite recall source {name}"))?;
        let column_count = statement.column_count();
        let mut rows = statement
            .query([])
            .with_context(|| format!("querying SQLite recall source {name}"))?;
        while let Some(row) = rows
            .next()
            .with_context(|| format!("reading SQLite recall source {name}"))?
        {
            for column in 0..column_count {
                let _: rusqlite::types::Value = row.get(column).with_context(|| {
                    format!("reading column {column} from SQLite recall source {name}")
                })?;
            }
        }
    }
    Ok(())
}

fn discover_markdown_roots(
    roots: &[NotesRootConfig],
    catalog: &mut BTreeMap<String, CatalogEntry>,
    filesystem: &mut BTreeMap<String, PathBuf>,
    folders: &mut MarkdownFolderEntityIndex,
) -> Result<()> {
    for discovered in FileDiscovery::from_notes_roots(roots.to_vec()).discover_named()? {
        let absolute = discovered.root_path.join(&discovered.relative_path);
        folders.observe(&discovered);
        let source_ref = discovered.source_ref;
        filesystem.insert(source_ref.clone(), absolute.clone());
        catalog.insert(
            source_ref,
            CatalogEntry {
                source: discovered.root_name,
                kind: SourceKind::Notes,
                evidence: EvidenceHandle::NativeMarkdown {
                    path: absolute.to_string_lossy().into_owned(),
                },
            },
        );
    }
    Ok(())
}

#[derive(Debug, Default)]
struct MarkdownFolderEntityIndex {
    home_by_scan_spec: BTreeMap<String, BTreeSet<String>>,
    indexed: BTreeSet<String>,
}

impl MarkdownFolderEntityIndex {
    fn observe(&mut self, discovered: &DiscoveredFile) {
        let relative = discovered
            .relative_path
            .to_string_lossy()
            .replace('\\', "/");
        let prefix = discovered
            .source_ref
            .strip_suffix(&relative)
            .unwrap_or("")
            .trim_end_matches('/');

        let parent = discovered
            .relative_path
            .parent()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if parent.is_empty() {
            self.push(".", folder_identity(prefix, "."), discovered.writable);
            return;
        }

        let mut current = String::new();
        for component in parent.split('/').filter(|component| !component.is_empty()) {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(&component.to_ascii_lowercase());
            self.push(
                &current,
                folder_identity(prefix, &current),
                discovered.writable,
            );
        }
    }

    fn push(&mut self, scan_spec_name: &str, indexed_name: String, writable: bool) {
        let key = scan_spec_name.to_ascii_lowercase();
        self.indexed.insert(indexed_name.clone());
        if writable {
            self.home_by_scan_spec
                .entry(key)
                .or_default()
                .insert(indexed_name);
        }
    }

    fn resolve_folder_ref(&self, entity_ref: &str) -> Result<Option<Vec<String>>> {
        let Some(folder_name) = prefixed_entity_name(entity_ref, "folder:") else {
            return Ok(None);
        };
        if self.indexed.contains(&folder_name) {
            return Ok(Some(vec![format!("folder:{folder_name}")]));
        }
        // `scan` examines the Workspace home. Its unscoped `folder:*` specs
        // therefore bind only to home Markdown identities; reference-source
        // curation needs a source-qualified folder ref instead of fan-out.
        let Some(matches) = self.home_by_scan_spec.get(&folder_name) else {
            anyhow::bail!(
                "configured folder entity {entity_ref:?} does not resolve to any indexed home Markdown source folder"
            );
        };
        anyhow::ensure!(
            matches.len() == 1,
            "configured folder entity {entity_ref:?} is ambiguous across indexed home Markdown source folders"
        );
        Ok(Some(
            matches
                .iter()
                .map(|indexed_name| format!("folder:{indexed_name}"))
                .collect(),
        ))
    }
}

/// Interim adapter until the engine reads the Workspace program directly: with
/// several Markdown sources, program folder readings are root-qualified
/// (`folder:<source>/<path>`). Map a Home-qualified folder to the Home scan
/// spec and another root's folder to its indexed identity.
fn engine_folder_ref(workspace: &ResolvedWorkspace, entity: &str) -> Result<String> {
    let markdown = workspace
        .config
        .bindings
        .iter()
        .filter_map(|(name, binding)| match binding {
            WorkspaceBinding::NativeMarkdown { path, role, .. } => Some((name, path, *role)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some(folder) = prefixed_entity_name(entity, "folder:") else {
        return Ok(entity.to_string());
    };
    if markdown.len() < 2 {
        return Ok(entity.to_string());
    }
    let (root, rest) = folder.split_once('/').unwrap_or((folder.as_str(), "."));
    let Some((_, path, role)) = markdown
        .iter()
        .find(|(name, _, _)| name.eq_ignore_ascii_case(root))
    else {
        return Ok(entity.to_string());
    };
    if *role == SourceRole::Home {
        return Ok(format!("folder:{rest}"));
    }
    Ok(format!(
        "folder:{}",
        folder_identity(
            &native_markdown_collection_namespace(path)?,
            &rest.to_ascii_lowercase()
        )
    ))
}

fn folder_identity(source_prefix: &str, folder_name: &str) -> String {
    match (source_prefix.is_empty(), folder_name == ".") {
        (true, true) => ".".to_string(),
        (true, false) => folder_name.to_string(),
        (false, true) => source_prefix.to_ascii_lowercase(),
        (false, false) => format!("{}/{folder_name}", source_prefix.to_ascii_lowercase()),
    }
}

/// Freeze the Workspace's ordinary entity surface. Operational approval is not
/// a corpus or catalyst-selection input. An explicit Workspace `entities` list
/// is authoritative. Otherwise automatic correspondents are ranked by their
/// rebuildable thread-association score, followed by native people-note
/// entities. Every entry still flows through the engine's existing entity
/// lookup, occurrence volume, era selection, and catalyst budgets.
struct CatalystEntityPolicy {
    entities: Vec<WorkspaceEntity>,
    excluded_links: BTreeSet<String>,
    debug: EntitySelectionDebug,
}

fn catalyst_entity_policy(
    workspace: &ResolvedWorkspace,
    markdown_folders: &MarkdownFolderEntityIndex,
) -> Result<CatalystEntityPolicy> {
    let excluded = workspace
        .config
        .policy
        .excluded_entities
        .iter()
        .map(|entity| normalize_entity_ref(entity))
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut entities = Vec::new();
    let mut selected_debug = Vec::new();
    fn push_entity(
        entity: String,
        options: Option<WorkspaceEntityOptions>,
        weight: Option<u64>,
        excluded: &BTreeSet<String>,
        markdown_folders: &MarkdownFolderEntityIndex,
        seen: &mut BTreeSet<String>,
        entities: &mut Vec<WorkspaceEntity>,
        selected_debug: &mut Vec<(String, Option<u64>)>,
    ) -> Result<bool> {
        let original_key = normalize_entity_ref(&entity);
        if original_key.is_empty() || excluded.contains(&original_key) {
            return Ok(false);
        }
        let values = markdown_folders
            .resolve_folder_ref(&entity)?
            .unwrap_or_else(|| vec![entity.trim().to_string()]);
        let mut pushed = false;
        for value in values {
            let key = normalize_entity_ref(&value);
            if !key.is_empty() && !excluded.contains(&key) && seen.insert(key) {
                selected_debug.push((value.clone(), weight));
                entities.push(match options.clone() {
                    Some(options) => WorkspaceEntity::with_options(value, options),
                    None => WorkspaceEntity::simple(value),
                });
                pushed = true;
            }
        }
        Ok(pushed)
    }

    if !workspace.config.policy.entities.is_empty() {
        for configured in &workspace.config.policy.entities {
            for (entity, options) in configured.entries() {
                push_entity(
                    engine_folder_ref(workspace, entity)?,
                    options.cloned(),
                    None,
                    &excluded,
                    markdown_folders,
                    &mut seen,
                    &mut entities,
                    &mut selected_debug,
                )?;
            }
        }
        return Ok(CatalystEntityPolicy {
            entities,
            excluded_links: BTreeSet::new(),
            debug: EntitySelectionDebug {
                selected: selected_debug,
                suppressed_count: 0,
                correspondents_considered: 0,
            },
        });
    }

    let owner_identities = workspace
        .config
        .bindings
        .values()
        .filter_map(WorkspaceBinding::gmail_account)
        .map(margins_workflows::integrations::normalize_email_identity)
        .filter(|identity| !identity.is_empty())
        .collect::<BTreeSet<_>>();
    let mut excluded_links = owner_identities.clone();
    let mut correspondents = Vec::new();
    let mut correspondents_considered = 0;
    for binding in workspace.config.bindings.values() {
        let Some(account) = binding.gmail_account() else {
            continue;
        };
        let selection = margins_workflows::integrations::automatic_correspondent_entity_selection(
            &workspace.state_dir,
            account,
        )?;
        correspondents_considered += selection.correspondents_considered;
        excluded_links.extend(selection.excluded_links);
        correspondents.extend(selection.entities);
    }
    correspondents.retain(|entity| !owner_identities.contains(&entity.identity_key));
    for entity in correspondents {
        if entities.len() >= ENGINE_ENTITY_LIMIT {
            break;
        }
        push_entity(
            entity.entity_ref(),
            None,
            Some(entity.weight),
            &excluded,
            markdown_folders,
            &mut seen,
            &mut entities,
            &mut selected_debug,
        )?;
    }
    for person in margins_workflows::session_index::recent_people_candidates(
        &workspace.home_dir,
        "people",
        None,
    ) {
        if entities.len() >= ENGINE_ENTITY_LIMIT {
            break;
        }
        let person = person.trim();
        if !person.is_empty() {
            push_entity(
                format!("[[{person}]]"),
                None,
                None,
                &excluded,
                markdown_folders,
                &mut seen,
                &mut entities,
                &mut selected_debug,
            )?;
        }
    }
    let suppressed_count = excluded_links.len();
    Ok(CatalystEntityPolicy {
        entities,
        excluded_links,
        debug: EntitySelectionDebug {
            selected: selected_debug,
            suppressed_count,
            correspondents_considered,
        },
    })
}

fn normalize_entity_ref(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn engine_entity_name(value: &str) -> Option<String> {
    let normalized = normalize_entity_ref(value);
    if let Some(name) = normalized
        .strip_prefix("[[")
        .and_then(|name| name.strip_suffix("]]"))
    {
        return (!name.trim().is_empty()).then(|| name.trim().to_string());
    }
    for prefix in ["#", "folder:", "log:"] {
        if let Some(name) = normalized.strip_prefix(prefix) {
            return (!name.trim().is_empty()).then(|| name.trim().to_string());
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

fn link_entity_name(value: &str) -> Option<String> {
    let normalized = normalize_entity_ref(value);
    normalized
        .strip_prefix("[[")
        .and_then(|value| value.strip_suffix("]]"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn prefixed_entity_name(value: &str, prefix: &str) -> Option<String> {
    normalize_entity_ref(value)
        .strip_prefix(prefix)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Build one stable source row per authoritative Calendar event. The engine
/// receives a generic document plus role-blind participant associations; it
/// never reads Calendar projections or raw transport payloads.
fn calendar_event_sqlite_source(
    db: &Path,
    connector: &str,
    account: &str,
    source_name: &str,
    scope: GoogleCalendarScope,
) -> EngineSource {
    let document_ref_prefix = format!("sqlite:{source_name}/");
    let query = format!(
        "SELECT ce.source_id AS id, \
         {} || lower(hex(CAST(ce.source_id AS BLOB))) AS document_ref, \
         CAST(strftime('%s', ce.occurred_from) AS INTEGER) * 1000 AS occurred_at_ms, \
         ce.title, ce.body_text AS body, \
         COALESCE((SELECT json_group_array(participant) FROM ( \
             SELECT COALESCE(NULLIF(ca.email, ''), ca.display_name) AS participant \
             FROM calendar_event_attendees ca \
             WHERE ca.connector_id = ce.connector_id \
               AND ca.source_account = ce.source_account \
               AND ca.source_id = ce.source_id \
             ORDER BY ca.position, ca.attendee_key \
         )), '[]') AS participants \
         FROM calendar_event_evidence ce \
         WHERE ce.connector_id = {} AND ce.source_account = {} \
           AND ce.tombstoned_at IS NULL \
           AND ce.occurred_from >= {} AND ce.occurred_from <= {}",
        sql_string(&document_ref_prefix),
        sql_string(connector),
        sql_string(account),
        sql_string(&scope.occurred_from.to_rfc3339()),
        sql_string(&scope.occurred_to.to_rfc3339()),
    );
    EngineSource::Sqlite {
        db: db.to_path_buf(),
        query,
        roles: EngineRoles {
            id: vec!["id".to_string()],
            document_ref: Some("document_ref".to_string()),
            who: EngineWhoRole {
                format: "json_array",
                column: "participants".to_string(),
            },
            when: "occurred_at_ms".to_string(),
            what: vec!["title".to_string(), "body".to_string()],
        },
        timestamp: EngineTimestamp { unit: "ms" },
    }
}

/// Build one stable source row per authoritative external evidence document.
/// Google Meet is the first concrete consumer. Provider-specific
/// attributes and raw transport payloads stay on the Margins side of the
/// boundary; Enzyme receives only generic document fields and role-blind
/// participant associations.
fn external_document_sqlite_source(
    db: &Path,
    connector: &str,
    account: &str,
    source_name: &str,
    occurred_from: Option<chrono::DateTime<chrono::Utc>>,
) -> EngineSource {
    let document_ref_prefix = format!("sqlite:{source_name}/");
    let time_predicate = occurred_from
        .map(|occurred_from| {
            format!(
                " AND ed.occurred_at >= {}",
                sql_string(&occurred_from.to_rfc3339())
            )
        })
        .unwrap_or_default();
    let query = format!(
        "SELECT ed.source_id AS id, \
         {} || lower(hex(CAST(ed.source_id AS BLOB))) AS document_ref, \
         CAST(strftime('%s', ed.occurred_at) AS INTEGER) * 1000 AS occurred_at_ms, \
         ed.title, ed.body_text AS body, \
         COALESCE((SELECT json_group_array(participant) FROM ( \
             SELECT COALESCE(NULLIF(ep.email, ''), ep.display_name) AS participant \
             FROM external_document_participants ep \
             WHERE ep.connector_id = ed.connector_id \
               AND ep.source_account = ed.source_account \
               AND ep.source_id = ed.source_id \
               AND ep.ambiguous = 0 \
             ORDER BY ep.position, ep.participant_key \
         )), '[]') AS participants \
         FROM external_document_evidence ed \
         WHERE ed.connector_id = {} AND ed.source_account = {} \
           AND ed.tombstoned_at IS NULL{}",
        sql_string(&document_ref_prefix),
        sql_string(connector),
        sql_string(account),
        time_predicate,
    );
    EngineSource::Sqlite {
        db: db.to_path_buf(),
        query,
        roles: EngineRoles {
            id: vec!["id".to_string()],
            document_ref: Some("document_ref".to_string()),
            who: EngineWhoRole {
                format: "json_array",
                column: "participants".to_string(),
            },
            when: "occurred_at_ms".to_string(),
            what: vec!["title".to_string(), "body".to_string()],
        },
        timestamp: EngineTimestamp { unit: "ms" },
    }
}

/// Build one stable source row per materialized email thread.
fn mail_thread_sqlite_source(
    db: &Path,
    connector: &str,
    account: &str,
    source_name: &str,
) -> EngineSource {
    let document_ref_prefix = format!("sqlite:{source_name}/");
    let query = format!(
        "SELECT te.thread_id AS id, \
         {} || lower(hex(CAST(te.thread_id AS BLOB))) AS document_ref, \
         CAST(strftime('%s', te.occurred_to) AS INTEGER) * 1000 AS occurred_at_ms, \
         te.body_text AS body, \
         COALESCE((
             SELECT json_group_array(ordered.participant)
             FROM (
                 SELECT DISTINCT pt.participant
                 FROM participant_threads pt
                 WHERE pt.connector_id = te.connector_id
                   AND pt.source_account = te.source_account
                   AND pt.thread_id = te.thread_id
                 ORDER BY pt.participant
             ) ordered
         ), '[]') AS participants \
         FROM thread_evidence te \
         WHERE te.connector_id = {} AND te.source_account = {} \
           AND te.tombstoned_at IS NULL",
        sql_string(&document_ref_prefix),
        sql_string(connector),
        sql_string(account),
    );
    EngineSource::Sqlite {
        db: db.to_path_buf(),
        query,
        roles: EngineRoles {
            id: vec!["id".to_string()],
            document_ref: Some("document_ref".to_string()),
            who: EngineWhoRole {
                format: "json_array",
                column: "participants".to_string(),
            },
            when: "occurred_at_ms".to_string(),
            what: vec!["body".to_string()],
        },
        timestamp: EngineTimestamp { unit: "ms" },
    }
}

fn load_ledger_catalog(
    workspace: &ResolvedWorkspace,
    catalog: &mut BTreeMap<String, CatalogEntry>,
) -> Result<()> {
    if !workspace.ledger_path().is_file() {
        return Ok(());
    }
    let connection = Connection::open_with_flags(
        workspace.ledger_path(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut thread_rows = connection.prepare(
        "SELECT connector_id, source_account, thread_id, href
         FROM thread_evidence WHERE tombstoned_at IS NULL
         ORDER BY connector_id, source_account, thread_id",
    )?;
    let rows = thread_rows.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (connector, account, source_id, href) = row?;
        let Some((name, binding)) = workspace.config.bindings.iter().find(|(_, binding)| {
            matches!(
                binding,
                WorkspaceBinding::Gmail {
                    account: declared,
                    ..
                } if connector == "email" && declared == &account
            )
        }) else {
            continue;
        };
        let document_ref = external_document_ref(name, binding, &source_id)?;
        catalog.insert(
            document_ref,
            CatalogEntry {
                source: name.clone(),
                kind: binding.kind(),
                evidence: external_evidence(
                    &connector,
                    &account,
                    &source_id,
                    href.unwrap_or_default(),
                ),
            },
        );
    }
    let mut calendar_rows = connection.prepare(
        "SELECT source_account, source_id, occurred_from, href
         FROM calendar_event_evidence
         WHERE connector_id = 'gcal' AND tombstoned_at IS NULL
         ORDER BY source_account, source_id",
    )?;
    let rows = calendar_rows.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    for row in rows {
        let (account, source_id, occurred_from, href) = row?;
        let Some((name, binding)) = workspace.config.bindings.iter().find(|(_, binding)| {
            matches!(
                binding,
                WorkspaceBinding::GoogleCalendar { account: declared, .. }
                    if declared == &account
            )
        }) else {
            continue;
        };
        let WorkspaceBinding::GoogleCalendar { calendar, .. } = binding else {
            unreachable!("Calendar binding matched above")
        };
        let occurred_from = chrono::DateTime::parse_from_rfc3339(&occurred_from)
            .context("Calendar evidence has invalid occurred_from")?
            .with_timezone(&chrono::Utc);
        let scope = GoogleCalendarScope::for_selector(calendar, chrono::Utc::now())?;
        if occurred_from < scope.occurred_from || occurred_from > scope.occurred_to {
            continue;
        }
        catalog.insert(
            external_document_ref(name, binding, &source_id)?,
            CatalogEntry {
                source: name.clone(),
                kind: binding.kind(),
                evidence: external_evidence("gcal", &account, &source_id, href.unwrap_or_default()),
            },
        );
    }
    let mut external_rows = connection.prepare(
        "SELECT connector_id, source_account, source_id, occurred_at, href
         FROM external_document_evidence
         WHERE tombstoned_at IS NULL
         ORDER BY connector_id, source_account, source_id",
    )?;
    let rows = external_rows.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    for row in rows {
        let (connector, account, source_id, _occurred_at, href) = row?;
        let Some((name, binding)) =
            workspace.config.bindings.iter().find(|(_, binding)| {
                binding_matches_external_document(binding, &connector, &account)
            })
        else {
            continue;
        };
        catalog.insert(
            external_document_ref(name, binding, &source_id)?,
            CatalogEntry {
                source: name.clone(),
                kind: binding.kind(),
                evidence: external_evidence(
                    &connector,
                    &account,
                    &source_id,
                    href.unwrap_or_default(),
                ),
            },
        );
    }
    Ok(())
}

fn external_evidence(
    connector_id: &str,
    source_account: &str,
    source_id: &str,
    href: String,
) -> EvidenceHandle {
    EvidenceHandle::ExternalRecord {
        connector_id: connector_id.to_string(),
        source_account: source_account.to_string(),
        source_id: source_id.to_string(),
        href: (!href.is_empty()).then_some(href),
    }
}

fn external_document_ref(
    binding_name: &str,
    binding: &WorkspaceBinding,
    id: &str,
) -> Result<String> {
    match binding {
        WorkspaceBinding::Gmail { account, .. } => Ok(sqlite_exact_document_ref(
            &gmail_collection_namespace(account)?,
            id,
        )),
        WorkspaceBinding::GoogleCalendar { account, .. } => Ok(sqlite_exact_document_ref(
            &calendar_collection_namespace(account)?,
            id,
        )),
        WorkspaceBinding::GoogleMeet { account } => Ok(sqlite_exact_document_ref(
            &meet_collection_namespace(account)?,
            id,
        )),
        WorkspaceBinding::Granola { account, .. } => Ok(sqlite_exact_document_ref(
            &granola_collection_namespace(account)?,
            id,
        )),
        _ => anyhow::bail!("binding {binding_name:?} has no external evidence document ref"),
    }
}

fn sqlite_exact_document_ref(source_name: &str, id: &str) -> String {
    let encoded_id = id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sqlite:{source_name}/{encoded_id}")
}

fn binding_matches_external_document(
    binding: &WorkspaceBinding,
    connector: &str,
    account: &str,
) -> bool {
    match binding {
        WorkspaceBinding::GoogleMeet { account: declared } => {
            connector == "google_meet" && declared == account
        }
        WorkspaceBinding::Granola {
            account: declared, ..
        } => connector == "granola" && declared == account,
        _ => false,
    }
}

pub(crate) fn path_modified_ms(path: &Path) -> Option<i64> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono::Utc;
    use margins_workflows::integrations::{
        ConnectorCtx, CurationObservation, IntegrationsStore, KindCounts, ParticipantThread,
        RawItemDraft, ThreadEvidence, EMAIL_CONNECTOR_ID,
    };
    use std::collections::BTreeMap;

    fn add_google_sources_for_test(
        workspace: &mut margins_workflows::workspace::ResolvedWorkspace,
        account: &str,
    ) {
        use margins_workflows::workspace::{
            add_source, CalendarCollectionSelector, GmailCollectionSelector, WorkspaceBinding,
        };
        for (name, binding) in [
            (
                "google-mail",
                WorkspaceBinding::Gmail {
                    account: account.to_string(),
                    gmail: GmailCollectionSelector::default_declaration(),
                },
            ),
            (
                "google-calendar",
                WorkspaceBinding::GoogleCalendar {
                    account: account.to_string(),
                    calendar: CalendarCollectionSelector::default_declaration(),
                },
            ),
            (
                "google-meet",
                WorkspaceBinding::GoogleMeet {
                    account: account.to_string(),
                },
            ),
        ] {
            add_source(workspace, name, binding).unwrap();
        }
    }

    fn seed_email_snapshot(
        store: &IntegrationsStore,
        ctx: &ConnectorCtx,
        threads: Vec<ThreadEvidence>,
        associations: Vec<ParticipantThread>,
    ) {
        store
            .replace_email_thread_snapshot(ctx, threads, associations)
            .unwrap();
    }

    #[test]
    fn native_document_refs_survive_binding_rename_and_second_root() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        let reference = temp.path().join("reference");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&reference).unwrap();
        std::fs::write(notes.join("home.md"), "# Home").unwrap();
        std::fs::write(reference.join("reference.md"), "# Reference").unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
                .unwrap();

        let first = prepare(&workspace, false)
            .unwrap()
            .filesystem_documents
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(first.len(), 1);
        let home_ref = first.iter().next().unwrap().clone();
        assert!(home_ref.starts_with("markdown_"));

        let home_binding = workspace.config.bindings.remove("home").unwrap();
        workspace
            .config
            .bindings
            .insert("renamed-home".to_string(), home_binding);
        let renamed = prepare(&workspace, false)
            .unwrap()
            .filesystem_documents
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(renamed, first);

        margins_workflows::workspace::add_source(
            &mut workspace,
            "research",
            WorkspaceBinding::NativeMarkdown {
                path: reference,
                role: SourceRole::Reference,
                note_folder: None,
            },
        )
        .unwrap();
        let expanded = prepare(&workspace, false)
            .unwrap()
            .filesystem_documents
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(expanded.len(), 2);
        assert!(expanded.contains(&home_ref));
    }

    #[test]
    fn managed_projection_tag_remains_excluded_after_binding_removal() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
                .unwrap();

        let corpus = prepare(&workspace, false).unwrap();
        let config: toml::Value =
            toml::from_str(&std::fs::read_to_string(&corpus.engine_config).unwrap()).unwrap();
        let excluded = config["workspaces"]["practice"]["excluded_tags"]
            .as_array()
            .unwrap();
        assert!(excluded
            .iter()
            .any(|value| value.as_str() == Some(MANAGED_PROJECTION_TAG)));
    }

    #[test]
    fn workspace_folder_entity_curation_resolves_to_native_markdown_identity() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        std::fs::write(notes.join("people/Alice.md"), "# Alice").unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
                .unwrap();
        workspace.config.policy.entities = vec![WorkspaceEntity::with_options(
            "folder:people",
            WorkspaceEntityOptions {
                profile: Some("relational".to_string()),
                expandable: true,
                children: Vec::new(),
            },
        )];

        let corpus = prepare(&workspace, false).unwrap();
        let folder_identity = format!(
            "{}/people",
            native_markdown_collection_namespace(&workspace.home_dir).unwrap()
        );
        let loaded = recall_engine::config::EnzymeConfig::load_at(
            &corpus.virtual_home,
            &corpus.engine_config,
        )
        .unwrap()
        .unwrap();
        let preferences = loaded.parse_preferences();

        assert_eq!(preferences.entities.len(), 1);
        assert_eq!(preferences.entities[0].name, folder_identity);
        assert_eq!(
            preferences
                .entity_profiles
                .get(&folder_identity)
                .map(String::as_str),
            Some("relational")
        );
        assert!(preferences.expandable_entities.contains(&folder_identity));
        assert!(corpus.selected_entity_names.contains(&folder_identity));
    }

    #[test]
    fn workspace_folder_entity_curation_targets_home_when_reference_source_matches() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let home = temp.path().join("home");
        let reference = temp.path().join("reference");
        std::fs::create_dir_all(home.join("people")).unwrap();
        std::fs::create_dir_all(reference.join("people")).unwrap();
        std::fs::write(home.join("people/Alice.md"), "# Alice").unwrap();
        std::fs::write(reference.join("people/Bob.md"), "# Bob").unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &home)
                .unwrap();
        margins_workflows::workspace::add_source(
            &mut workspace,
            "reference",
            WorkspaceBinding::NativeMarkdown {
                path: reference.clone(),
                role: SourceRole::Reference,
                note_folder: None,
            },
        )
        .unwrap();
        workspace.config.policy.entities = vec![WorkspaceEntity::with_options(
            "folder:people",
            WorkspaceEntityOptions {
                profile: Some("relational".to_string()),
                expandable: true,
                children: Vec::new(),
            },
        )];

        let corpus = prepare(&workspace, false).unwrap();
        let expected = BTreeSet::from([format!(
            "{}/people",
            native_markdown_collection_namespace(&workspace.home_dir).unwrap()
        )]);

        assert_eq!(corpus.selected_entity_names, expected);
        let loaded = recall_engine::config::EnzymeConfig::load_at(
            &corpus.virtual_home,
            &corpus.engine_config,
        )
        .unwrap()
        .unwrap();
        let preferences = loaded.parse_preferences();
        assert_eq!(
            preferences
                .entities
                .iter()
                .map(|entity| entity.name.clone())
                .collect::<BTreeSet<_>>(),
            expected
        );
        for identity in expected {
            assert_eq!(
                preferences
                    .entity_profiles
                    .get(&identity)
                    .map(String::as_str),
                Some("relational")
            );
            assert!(preferences.expandable_entities.contains(&identity));
        }
    }

    #[test]
    fn workspace_folder_entity_curation_rejects_reference_only_unscoped_folder() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let home = temp.path().join("home");
        let reference = temp.path().join("reference");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(reference.join("people")).unwrap();
        std::fs::write(home.join("home.md"), "# Home").unwrap();
        std::fs::write(reference.join("people/Bob.md"), "# Bob").unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &home)
                .unwrap();
        margins_workflows::workspace::add_source(
            &mut workspace,
            "reference",
            WorkspaceBinding::NativeMarkdown {
                path: reference,
                role: SourceRole::Reference,
                note_folder: None,
            },
        )
        .unwrap();
        workspace.config.policy.entities = vec![WorkspaceEntity::simple("folder:people")];

        let error = prepare(&workspace, false).unwrap_err().to_string();
        assert!(
            error.contains("does not resolve to any indexed home Markdown source folder"),
            "{error}"
        );
    }

    #[test]
    fn automatic_human_correspondent_enters_selection_without_approval_but_noise_does_not() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        std::fs::write(notes.join("people/Note Curated.md"), "# Note Curated").unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
                .unwrap();
        add_google_sources_for_test(&mut workspace, "owner@example.com");
        let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
        let ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        let human_at = Utc.with_ymd_and_hms(2026, 8, 21, 10, 0, 0).unwrap();
        let newsletter_at = Utc.with_ymd_and_hms(2026, 8, 21, 11, 0, 0).unwrap();
        seed_email_snapshot(
            &store,
            &ctx,
            vec![
                ThreadEvidence {
                    thread_id: "human-thread".into(),
                    occurred_from: human_at,
                    occurred_to: human_at,
                    body_text: "Can we decide the project direction?".into(),
                    href: None,
                },
                ThreadEvidence {
                    thread_id: "newsletter-thread".into(),
                    occurred_from: newsletter_at,
                    occurred_to: newsletter_at,
                    body_text: "Automated daily digest.".into(),
                    href: None,
                },
            ],
            vec![
                ParticipantThread {
                    participant: "human@client.test".into(),
                    thread_id: "human-thread".into(),
                    last_interaction: human_at,
                    sampling_score: Some(1_001_001),
                },
                ParticipantThread {
                    participant: "newsletter@noise.test".into(),
                    thread_id: "newsletter-thread".into(),
                    last_interaction: newsletter_at,
                    sampling_score: Some(1),
                },
            ],
        );
        store
            .replace_email_thread_snapshot_with_policy_report(
                &ctx,
                store.thread_evidence(&ctx).unwrap(),
                store.participant_threads(&ctx).unwrap(),
                &CurationObservation {
                    connector_id: EMAIL_CONNECTOR_ID.to_string(),
                    account: ctx.account.clone(),
                    item_counts: KindCounts::default(),
                    occurred_from: None,
                    occurred_to: None,
                    observation_window: None,
                    observed_range: None,
                    sample_observed_ranges: BTreeMap::new(),
                    observation_call_count: None,
                    detected_accounts: vec![ctx.account.clone()],
                    inferred_people: Vec::new(),
                    inferred_orgs: Vec::new(),
                    proposed_include: vec![
                        "person:human@client.test".into(),
                        "domain:client.test".into(),
                        "person:newsletter@noise.test".into(),
                        "domain:noise.test".into(),
                    ],
                    proposed_exclude: Vec::new(),
                    proposal_flags: BTreeMap::from([
                        (
                            "person:newsletter@noise.test".into(),
                            vec!["likely-automated".into()],
                        ),
                        ("domain:noise.test".into(), vec!["likely-automated".into()]),
                    ]),
                    proposal_evidence: BTreeMap::new(),
                    decisions_required: Vec::new(),
                },
                None,
                "practice-test",
                Utc::now(),
                None,
                None,
            )
            .unwrap();

        let corpus = prepare(&workspace, true).unwrap();
        let config = std::fs::read_to_string(&corpus.engine_config).unwrap();
        assert!(config.contains("[[human@client.test]]"));
        assert!(!config.contains("[[client.test]]"));
        assert!(config.contains("[[Note Curated]]"));
        assert!(config.contains("newsletter@noise.test"));
        assert!(!config.contains("[[noise.test]]"));
    }

    #[test]
    fn round8_ephemeral_config_owns_weighted_selection_and_hard_exclusions() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "round8", None, &notes)
                .unwrap();
        add_google_sources_for_test(&mut workspace, "owner@example.com");
        let store = IntegrationsStore::open(&workspace.state_dir).unwrap();
        let ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        let occurred = Utc.with_ymd_and_hms(2026, 8, 21, 10, 0, 0).unwrap();
        let reply_at = Utc.with_ymd_and_hms(2026, 8, 21, 10, 15, 0).unwrap();
        let mut threads = Vec::new();
        let mut associations = Vec::new();
        let shared_score = 1_001_001_u64;
        for index in 0..10 {
            let thread_id = format!("shared-{index:02}");
            threads.push(ThreadEvidence {
                thread_id: thread_id.clone(),
                occurred_from: occurred,
                occurred_to: reply_at,
                body_text: "Retained evidence".into(),
                href: None,
            });
            for participant in ["bob.shared@outlook.com", "dkshared@gmail.com"] {
                associations.push(ParticipantThread {
                    participant: participant.into(),
                    thread_id: thread_id.clone(),
                    last_interaction: reply_at,
                    sampling_score: Some(shared_score),
                });
            }
        }
        for index in 0..51 {
            let thread_id = format!("ci-{index:02}");
            threads.push(ThreadEvidence {
                thread_id: thread_id.clone(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "CI notification".into(),
                href: None,
            });
            associations.push(ParticipantThread {
                participant: "ci_activity@noreply.github.com".into(),
                thread_id,
                last_interaction: occurred,
                sampling_score: Some(1),
            });
        }
        for index in 0..4 {
            let thread_id = format!("marketing-{index:02}");
            threads.push(ThreadEvidence {
                thread_id: thread_id.clone(),
                occurred_from: occurred,
                occurred_to: occurred,
                body_text: "Campaign".into(),
                href: None,
            });
            associations.push(ParticipantThread {
                participant: "offers@campaign.test".into(),
                thread_id,
                last_interaction: occurred,
                sampling_score: Some(1),
            });
        }
        seed_email_snapshot(&store, &ctx, threads, associations);
        store
            .replace_email_thread_snapshot_with_policy_report(
                &ctx,
                store.thread_evidence(&ctx).unwrap(),
                store.participant_threads(&ctx).unwrap(),
                &CurationObservation {
                    connector_id: EMAIL_CONNECTOR_ID.to_string(),
                    account: ctx.account.clone(),
                    item_counts: KindCounts::default(),
                    occurred_from: None,
                    occurred_to: None,
                    observation_window: None,
                    observed_range: None,
                    sample_observed_ranges: BTreeMap::new(),
                    observation_call_count: None,
                    detected_accounts: vec![ctx.account.clone()],
                    inferred_people: Vec::new(),
                    inferred_orgs: Vec::new(),
                    proposed_include: vec![
                        "person:ci_activity@noreply.github.com".into(),
                        "domain:noreply.github.com".into(),
                        "person:offers@campaign.test".into(),
                        "domain:campaign.test".into(),
                        "domain:gmail.com".into(),
                        "domain:outlook.com".into(),
                    ],
                    proposed_exclude: Vec::new(),
                    proposal_flags: BTreeMap::from([
                        (
                            "person:ci_activity@noreply.github.com".into(),
                            vec!["likely-automated".into()],
                        ),
                        (
                            "domain:noreply.github.com".into(),
                            vec!["likely-automated".into()],
                        ),
                        (
                            "person:offers@campaign.test".into(),
                            vec!["likely-automated".into()],
                        ),
                        (
                            "domain:campaign.test".into(),
                            vec!["likely-automated".into()],
                        ),
                    ]),
                    proposal_evidence: BTreeMap::new(),
                    decisions_required: Vec::new(),
                },
                None,
                "round8-test",
                Utc::now(),
                None,
                None,
            )
            .unwrap();

        let corpus = prepare(&workspace, true).unwrap();
        let config: toml::Value =
            toml::from_str(&std::fs::read_to_string(&corpus.engine_config).unwrap()).unwrap();
        let workspace_config = &config["workspaces"]["round8"];
        let entities = workspace_config["entities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            entities,
            vec!["[[bob.shared@outlook.com]]", "[[dkshared@gmail.com]]"],
            "the exact engine entity list is weighted human correspondence only"
        );
        let excluded_links = workspace_config["excluded_links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<BTreeSet<_>>();
        for excluded in [
            "owner@example.com",
            "ci_activity@noreply.github.com",
            "noreply.github.com",
            "offers@campaign.test",
            "campaign.test",
            "gmail.com",
            "outlook.com",
        ] {
            assert!(
                excluded_links.contains(excluded),
                "missing hard exclusion {excluded}"
            );
        }
        assert_eq!(config["defaults"]["total_limit"].as_integer(), Some(30));
        let mail_source = gmail_collection_namespace("owner@example.com").unwrap();
        let mail_roles = &workspace_config["sources"][mail_source.as_str()]["roles"];
        assert_eq!(mail_roles["who"]["format"].as_str(), Some("json_array"));
        assert_eq!(mail_roles["who"]["column"].as_str(), Some("participants"));
        assert_eq!(
            mail_roles["what"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["body"]
        );
        assert!(mail_roles.get("weight").is_none());
        let mail_query = workspace_config["sources"][mail_source.as_str()]["query"]
            .as_str()
            .unwrap();
        assert!(!mail_query.contains("sampling_score"));
        assert!(!mail_query.contains(" AS title"));
        assert!(
            !std::fs::read_to_string(&corpus.engine_config)
                .unwrap()
                .contains("who_00"),
            "the fixed participant-column bridge must stay deleted"
        );
        assert_eq!(corpus.entity_selection_debug.selected.len(), 2);
        assert_eq!(
            corpus.entity_selection_debug.selected[0].1,
            Some(10_010_010)
        );
    }

    #[test]
    fn raw_connector_cache_never_materializes_recall_entities() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let account = "owner.round9@gmail.com";
        let mut workspace = margins_workflows::workspace::create_workspace(
            &margins_home,
            "cache-only",
            None,
            &notes,
        )
        .unwrap();
        add_google_sources_for_test(&mut workspace, account);
        let ctx = ConnectorCtx {
            vault_root: workspace.state_dir.clone(),
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            account: account.to_string(),
            command_path: None,
        };
        IntegrationsStore::open(&workspace.state_dir)
            .unwrap()
            .ingest_raw_connector_items(
                &ctx,
                vec![RawItemDraft {
                    source_id: "cache-only".to_string(),
                    payload: serde_json::json!({"connector_cache": "not evidence"}),
                }],
            )
            .unwrap();

        let corpus = prepare(&workspace, true).unwrap();
        assert_eq!(corpus.entity_selection_debug.correspondents_considered, 0);
        let config: toml::Value =
            toml::from_str(&std::fs::read_to_string(&corpus.engine_config).unwrap()).unwrap();
        assert_eq!(
            config["workspaces"]["cache-only"]["entities"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![EMPTY_LEDGER_SELECTION_SENTINEL]
        );
        let loaded = recall_engine::config::EnzymeConfig::load_at(
            &corpus.virtual_home,
            &corpus.engine_config,
        )
        .unwrap()
        .unwrap();
        assert!(
            loaded.has_curated_entities(),
            "the internal sentinel must prevent engine coverage fallback"
        );
        let excluded = config["workspaces"]["cache-only"]["excluded_links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert!(excluded.contains("owner.round9@gmail.com"));
        assert!(excluded.contains("ownerround9@gmail.com"));
    }
}
