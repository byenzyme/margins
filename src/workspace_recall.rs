//! Margins' side of Workspace recall: turn a Workspace program into the
//! host-lowered program the engine indexes, and map engine document refs back
//! to Margins provenance (Sources, native paths, external records).
//!
//! The engine call itself lives in [`crate::recall_engine_seam`]; this module
//! only produces plain data for it.

use anyhow::{Context, Result};
use margins_workflows::enzyme_spec;
use margins_workflows::integrations::{EvidenceHandle, GoogleCalendarScope};
use margins_workflows::workspace::{ResolvedWorkspace, SourceKind, WorkspaceBinding};
use margins_workflows::workspace_lowering::{lower_for_engine, sqlite_document_ref};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::recall_engine_seam::{self, EngineWorkspace};

#[derive(Debug, Clone)]
pub(crate) struct CatalogEntry {
    pub source: String,
    pub kind: SourceKind,
    pub evidence: EvidenceHandle,
}

#[derive(Debug)]
pub(crate) struct WorkspaceCorpus {
    /// Exactly what the engine receives.
    pub engine: EngineWorkspace,
    pub catalog: BTreeMap<String, CatalogEntry>,
    pub filesystem_documents: BTreeMap<String, PathBuf>,
    /// Lowered SQLite source name -> database it reads.
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

pub(crate) const ENGINE_ENTITY_LIMIT: usize = 30;
const EMPTY_LEDGER_SELECTION_SENTINEL: &str = "[[margins:empty-ledger-selection]]";

/// Build the Workspace's engine input and provenance catalog.
///
/// The program is the Workspace's own program, host-lowered
/// ([`lower_for_engine`]) and extended with Margins' hidden selection rules:
/// when the program declares no readings, automatic correspondents and recent
/// people notes become link readings, and correspondence noise becomes
/// `leave out links`. None of it is written to the user's program file.
///
/// `for_indexing` additionally creates an empty ledger for declared ledger
/// sources that have never synced (so their lowered queries run and return no
/// rows) and preflights every lowered SQLite query.
pub(crate) fn prepare(workspace: &ResolvedWorkspace, for_indexing: bool) -> Result<WorkspaceCorpus> {
    let ledger = workspace.ledger_path();
    let has_ledger_source = workspace.config.bindings.values().any(is_ledger_binding);
    if for_indexing && has_ledger_source && !ledger.is_file() {
        margins_workflows::integrations::IntegrationsStore::open(&workspace.state_dir)
            .context("creating the Workspace ledger for declared sources")?;
    }
    let mut program = lower_for_engine(workspace.program.program(), &ledger, chrono::Utc::now())?;
    // Margins' catalyst budget, independent of any machine Enzyme settings.
    program.settings.total_limit = Some(ENGINE_ENTITY_LIMIT);
    let entity_policy = catalyst_entity_policy(workspace)?;
    {
        let body = &mut program.workspaces[0];
        if body.readings.is_empty() {
            let mut readings = entity_policy
                .automatic
                .iter()
                .map(|entity| link_reading(entity))
                .collect::<Vec<_>>();
            if has_ledger_source && readings.is_empty() {
                // The engine treats an empty reading list as "no curated
                // policy" and falls back to raw occurrence coverage. Without
                // this sentinel a decode failure or all-noise mailbox would
                // turn into broad catalyst generation.
                readings.push(link_reading(EMPTY_LEDGER_SELECTION_SENTINEL));
            }
            body.readings = readings;
        }
        for link in &entity_policy.excluded_links {
            if !body
                .excluded_links
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(link))
            {
                body.excluded_links.push(link.clone());
            }
        }
    }
    let excluded_link_entities = program.workspaces[0]
        .excluded_links
        .iter()
        .map(|link| normalize_entity_ref(link))
        .collect::<BTreeSet<_>>();
    if let Some(margins_home) = workspace.state_dir.parent().and_then(Path::parent) {
        if let Some(shared) = margins_workflows::workspace::shared_profiles(margins_home)? {
            for (name, profile) in shared.profiles {
                program.profiles.entry(name).or_insert(profile);
            }
        }
    }
    let sqlite_sources = program.workspaces[0]
        .sources
        .iter()
        .filter_map(|source| match source {
            enzyme_spec::Source::Sqlite(sqlite) => Some(sqlite.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if for_indexing {
        validate_sqlite_sources(&sqlite_sources)?;
    }
    let engine = EngineWorkspace {
        program_text: enzyme_spec::render_program(&program),
        workspace: workspace.config.id.clone(),
        state_dir: workspace.state_dir.clone(),
    };

    let mut catalog = BTreeMap::new();
    let mut filesystem_documents = BTreeMap::new();
    for document in recall_engine_seam::markdown_documents(&engine)? {
        filesystem_documents.insert(document.source_ref.clone(), document.path.clone());
        catalog.insert(
            document.source_ref,
            CatalogEntry {
                source: document.source,
                kind: SourceKind::Notes,
                evidence: EvidenceHandle::NativeMarkdown {
                    path: document.path.to_string_lossy().into_owned(),
                },
            },
        );
    }
    load_ledger_catalog(workspace, &mut catalog)?;

    let curated = recall_engine_seam::curated_entities(&engine)?;
    let sentinel = engine_entity_name(EMPTY_LEDGER_SELECTION_SENTINEL);
    let selected_link_entities = curated
        .iter()
        .filter(|(_, kind)| kind == "link")
        .map(|(name, _)| name.clone())
        .collect();
    let selected_entity_names = curated
        .into_iter()
        .map(|(name, _)| name)
        .filter(|name| Some(name) != sentinel.as_ref())
        .collect();
    Ok(WorkspaceCorpus {
        engine,
        catalog,
        filesystem_documents,
        sqlite_sources: if ledger.is_file() {
            sqlite_sources
                .iter()
                .map(|source| (source.name.clone(), PathBuf::from(&source.db)))
                .collect()
        } else {
            BTreeMap::new()
        },
        entity_selection_debug: entity_policy.debug,
        excluded_link_entities,
        selected_link_entities,
        selected_entity_names,
    })
}

fn is_ledger_binding(binding: &WorkspaceBinding) -> bool {
    matches!(
        binding,
        WorkspaceBinding::Gmail { .. }
            | WorkspaceBinding::GoogleCalendar { .. }
            | WorkspaceBinding::GoogleMeet { .. }
            | WorkspaceBinding::Granola { .. }
    )
}

fn link_reading(entity: &str) -> enzyme_spec::Reading {
    enzyme_spec::Reading {
        entity: entity.trim().to_string(),
        profile: "auto".to_string(),
        definition: None,
        learning: enzyme_spec::Learning::default(),
        include_linked_pages: false,
        include_who_links: false,
        pattern: false,
    }
}

/// Exhaust every lowered SQLite query before the engine receives the existing
/// index. A bad ledger query must not let a refresh prune documents before the
/// source error is discovered.
fn validate_sqlite_sources(sources: &[enzyme_spec::SqliteSource]) -> Result<()> {
    for source in sources {
        let name = &source.name;
        let connection = Connection::open_with_flags(
            &source.db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .with_context(|| format!("opening SQLite recall source {name} at {}", source.db))?;
        let mut statement = connection
            .prepare(&source.query)
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

/// Automatic catalyst selection for a program that declares no readings.
/// Operational approval is not a corpus or catalyst-selection input.
/// Correspondents are ranked by their rebuildable thread-association score,
/// followed by native people-note entities. Every entry still flows through
/// the engine's entity lookup, occurrence volume, era selection, and budgets.
struct CatalystEntityPolicy {
    automatic: Vec<String>,
    excluded_links: BTreeSet<String>,
    debug: EntitySelectionDebug,
}

fn catalyst_entity_policy(workspace: &ResolvedWorkspace) -> Result<CatalystEntityPolicy> {
    let curated = workspace.program.workspace().readings.clone();
    if !curated.is_empty() {
        return Ok(CatalystEntityPolicy {
            automatic: Vec::new(),
            excluded_links: BTreeSet::new(),
            debug: EntitySelectionDebug {
                selected: curated
                    .iter()
                    .filter(|reading| !reading.pattern)
                    .map(|reading| (reading.entity.clone(), None))
                    .collect(),
                suppressed_count: 0,
                correspondents_considered: 0,
            },
        });
    }
    let excluded = workspace
        .config
        .policy
        .excluded_entities
        .iter()
        .map(|entity| normalize_entity_ref(entity))
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut automatic = Vec::new();
    let mut selected_debug = Vec::new();
    let mut push = |entity: String, weight: Option<u64>, automatic: &mut Vec<String>| {
        let key = normalize_entity_ref(&entity);
        if !key.is_empty() && !excluded.contains(&key) && seen.insert(key) {
            selected_debug.push((entity.trim().to_string(), weight));
            automatic.push(entity.trim().to_string());
        }
    };

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
        if automatic.len() >= ENGINE_ENTITY_LIMIT {
            break;
        }
        push(entity.entity_ref(), Some(entity.weight), &mut automatic);
    }
    for person in margins_workflows::session_index::recent_people_candidates(
        &workspace.home_dir,
        "people",
        None,
    ) {
        if automatic.len() >= ENGINE_ENTITY_LIMIT {
            break;
        }
        let person = person.trim();
        if !person.is_empty() {
            push(format!("[[{person}]]"), None, &mut automatic);
        }
    }
    let suppressed_count = excluded_links.len();
    Ok(CatalystEntityPolicy {
        automatic,
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
    normalized
        .strip_prefix("[[")
        .and_then(|name| name.strip_suffix("]]"))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// Markdown identity under the Workspace language: with one Markdown source
/// refs are root-relative; with several they are `<source name>/<relative>`.
/// Returns the declaring source name and the absolute path of a native ref.
pub(crate) fn markdown_document(
    workspace: &ResolvedWorkspace,
    document_ref: &str,
) -> Option<(String, PathBuf)> {
    if document_ref.starts_with("sqlite:") {
        return None;
    }
    let markdown = workspace
        .config
        .bindings
        .iter()
        .filter_map(|(name, binding)| match binding {
            WorkspaceBinding::NativeMarkdown { path, .. } => Some((name, path)),
            _ => None,
        })
        .collect::<Vec<_>>();
    match markdown.as_slice() {
        [] => None,
        [(name, path)] => Some(((*name).clone(), path.join(document_ref))),
        many => {
            let (root, relative) = document_ref.split_once('/')?;
            many.iter()
                .find(|(name, _)| name.as_str() == root)
                .map(|(name, path)| ((*name).clone(), path.join(relative)))
        }
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
        catalog.insert(
            sqlite_document_ref(name, &source_id),
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
            sqlite_document_ref(name, &source_id),
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
            sqlite_document_ref(name, &source_id),
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono::Utc;
    use margins_workflows::integrations::{
        ConnectorCtx, CurationObservation, IntegrationsStore, KindCounts, ParticipantThread,
        RawItemDraft, ThreadEvidence, EMAIL_CONNECTOR_ID,
    };
    use margins_workflows::workspace::{SourceRole, WorkspaceEntity, WorkspaceEntityOptions};
    use margins_workflows::workspace_lowering::MANAGED_PROJECTION_TAG;
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

    fn engine_program(corpus: &WorkspaceCorpus) -> enzyme_spec::Workspace {
        enzyme_spec::parse(&corpus.engine.program_text)
            .unwrap()
            .workspaces
            .remove(0)
    }

    fn readings(corpus: &WorkspaceCorpus) -> Vec<String> {
        engine_program(corpus)
            .readings
            .into_iter()
            .map(|reading| reading.entity)
            .collect()
    }

    fn set_entities(workspace: &mut ResolvedWorkspace, entities: Vec<WorkspaceEntity>) {
        let mut policy = workspace.config.policy.clone();
        policy.entities = entities;
        margins_workflows::workspace::update_policy(workspace, policy).unwrap();
    }

    #[test]
    fn markdown_document_identity_follows_the_workspace_language() {
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
        let home_name = workspace
            .config
            .bindings
            .iter()
            .find(|(_, binding)| matches!(binding, WorkspaceBinding::NativeMarkdown { .. }))
            .map(|(name, _)| name.clone())
            .unwrap();

        // One Markdown source: root-relative refs.
        let single = prepare(&workspace, false).unwrap();
        assert_eq!(
            single.filesystem_documents.keys().collect::<Vec<_>>(),
            ["home.md"]
        );
        assert_eq!(single.catalog["home.md"].source, home_name);
        assert_eq!(
            markdown_document(&workspace, "home.md"),
            Some((home_name.clone(), notes.join("home.md")))
        );

        // Several Markdown sources: refs are qualified by the source name.
        margins_workflows::workspace::add_source(
            &mut workspace,
            "research",
            WorkspaceBinding::NativeMarkdown {
                path: reference.clone(),
                role: SourceRole::Reference,
                note_folder: None,
            },
        )
        .unwrap();
        let multi = prepare(&workspace, false).unwrap();
        let home_ref = format!("{home_name}/home.md");
        assert_eq!(
            multi.filesystem_documents.keys().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([home_ref.clone(), "research/reference.md".to_string()])
        );
        assert_eq!(multi.catalog["research/reference.md"].source, "research");
        assert_eq!(
            markdown_document(&workspace, "research/reference.md"),
            Some(("research".to_string(), reference.join("reference.md")))
        );
        assert_eq!(markdown_document(&workspace, "sqlite:mail/00"), None);
    }

    #[test]
    fn managed_projection_tag_is_a_hidden_lowering_rule() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
                .unwrap();

        let corpus = prepare(&workspace, false).unwrap();
        assert!(engine_program(&corpus)
            .excluded_tags
            .contains(&MANAGED_PROJECTION_TAG.to_string()));
        assert!(!workspace.program.text().contains(MANAGED_PROJECTION_TAG));
        assert!(!std::fs::read_to_string(&workspace.config_path)
            .unwrap()
            .contains(MANAGED_PROJECTION_TAG));
    }

    #[test]
    fn folder_reading_resolves_through_the_engine_with_profile() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins");
        let notes = temp.path().join("notes");
        std::fs::create_dir_all(notes.join("people")).unwrap();
        std::fs::write(notes.join("people/Alice.md"), "# Alice").unwrap();
        let mut workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
                .unwrap();
        set_entities(
            &mut workspace,
            vec![WorkspaceEntity::with_options(
                "folder:people",
                WorkspaceEntityOptions {
                    profile: Some("relational".to_string()),
                    expandable: true,
                    children: Vec::new(),
                },
            )],
        );

        let corpus = prepare(&workspace, false).unwrap();
        assert_eq!(readings(&corpus), ["folder:people"]);
        assert!(engine_program(&corpus).readings[0].include_linked_pages);
        assert_eq!(
            corpus.selected_entity_names,
            BTreeSet::from(["people".to_string()])
        );
        assert!(corpus.selected_link_entities.is_empty());
    }

    #[test]
    fn folder_reading_is_root_qualified_with_several_markdown_sources() {
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
                path: reference,
                role: SourceRole::Reference,
                note_folder: None,
            },
        )
        .unwrap();
        set_entities(
            &mut workspace,
            vec![WorkspaceEntity::simple("folder:reference/people")],
        );

        let corpus = prepare(&workspace, false).unwrap();
        assert_eq!(readings(&corpus), ["folder:reference/people"]);
        assert_eq!(
            corpus.selected_entity_names,
            BTreeSet::from(["reference/people".to_string()])
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
        let selected = readings(&corpus);
        assert!(selected.contains(&"[[human@client.test]]".to_string()), "{selected:?}");
        assert!(!selected.contains(&"[[client.test]]".to_string()));
        assert!(selected.contains(&"[[Note Curated]]".to_string()));
        assert!(!selected.contains(&"[[newsletter@noise.test]]".to_string()));
        let excluded = engine_program(&corpus).excluded_links;
        assert!(excluded.contains(&"newsletter@noise.test".to_string()), "{excluded:?}");
        assert!(!selected.contains(&"[[noise.test]]".to_string()));
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
        assert_eq!(
            readings(&corpus),
            vec!["[[bob.shared@outlook.com]]", "[[dkshared@gmail.com]]"],
            "the exact engine reading list is weighted human correspondence only"
        );
        let body = engine_program(&corpus);
        let excluded_links = body.excluded_links.iter().cloned().collect::<BTreeSet<_>>();
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
        let program = enzyme_spec::parse(&corpus.engine.program_text).unwrap();
        assert_eq!(program.settings.total_limit, Some(ENGINE_ENTITY_LIMIT));
        let mail = body
            .sources
            .iter()
            .find_map(|source| match source {
                enzyme_spec::Source::Sqlite(sqlite) if sqlite.name == "google-mail" => {
                    Some(sqlite.clone())
                }
                _ => None,
            })
            .expect("google-mail lowers to a SQLite source of the same name");
        assert_eq!(
            mail.who,
            enzyme_spec::SqliteWho::JsonArray {
                column: "participants".to_string()
            }
        );
        assert_eq!(mail.what, vec!["body"]);
        assert!(mail.weight.is_none());
        assert!(!mail.query.contains("sampling_score"));
        assert!(!mail.query.contains(" AS title"));
        assert!(
            !corpus.engine.program_text.contains("who_00"),
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
        assert_eq!(readings(&corpus), vec![EMPTY_LEDGER_SELECTION_SENTINEL]);
        assert!(
            !recall_engine_seam::curated_entities(&corpus.engine)
                .unwrap()
                .is_empty(),
            "the internal sentinel must prevent engine coverage fallback"
        );
        assert!(corpus.selected_entity_names.is_empty());
        let excluded = engine_program(&corpus)
            .excluded_links
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert!(excluded.contains("owner.round9@gmail.com"));
        assert!(excluded.contains("ownerround9@gmail.com"));
    }
}
