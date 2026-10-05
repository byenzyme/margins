//! The one hand-off between Margins and the recall engine for indexing and
//! opening a Workspace index.
//!
//! Inputs are plain data: the resolved, host-lowered Workspace program *text*
//! (`enzyme_spec::render_program` output), the workspace name, the Workspace
//! state directory, and generator settings (the Margins home whose catalyst
//! selection and cached hosted bundle choose the generator). Inside, the text is
//! parsed and resolved with `enzyme-spec` and handed to the engine as an
//! in-memory configuration (`EnzymeConfig::from_program_workspace`). The engine
//! never reads a Margins file, `ENZYME_HOME`, or `~/.enzyme`.
//!
//! No other Margins module constructs engine configuration types. The engine
//! is expected to move into its own process later; this module is that seam.
//! Search queries against an opened [`SearchIndex`] and read-only scan helpers
//! still call the engine directly (see `recall.rs` / `scan.rs`).

use anyhow::{Context, Result};
use margins_workflows::enzyme_spec;
use recall_engine::config::EnzymeConfig;
use recall_engine::document::FileDiscovery;
use recall_engine::kernel::{ensure_searchable_sync, Generator, SearchIndex, SearchOptions};
use recall_engine::llm::ConfigProvider;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Document identity written by this binary. Markdown refs follow the
/// Workspace language: a lone Markdown source's notes are root-relative; with
/// several Markdown sources they are qualified by the source *name*. Ledger
/// sources are `sqlite:<source name>/<hex id>`. An index without this marker
/// (built by an earlier release with hashed `markdown_<…>` / `gmail_<…>`
/// namespaces) is fully reindexed exactly once.
pub(crate) const DOCUMENT_IDENTITY: &str = "margins.document-identity.v2:source-name";
const IDENTITY_MARKER: &str = "index.identity";

/// A Workspace as the engine sees it: plain data only.
#[derive(Debug, Clone)]
pub(crate) struct EngineWorkspace {
    /// Host-lowered program text containing the `workspace "<name>"` block
    /// (and any profiles it uses).
    pub program_text: String,
    pub workspace: String,
    /// `$MARGINS_HOME/workspaces/<id>`; holds `index.db`.
    pub state_dir: PathBuf,
}

impl EngineWorkspace {
    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("index.db")
    }
}

/// One Markdown document exactly as the engine will index it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkdownDocument {
    /// Declaring Markdown source name.
    pub source: String,
    /// Engine document ref (the index's `docs.source_ref`).
    pub source_ref: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct ProvisionRequest {
    pub max_bridged_entities: usize,
    /// Link entities still selected; catalysts for other links are dropped.
    pub selected_link_entities: BTreeSet<String>,
    /// Link entities whose catalysts are always dropped.
    pub excluded_link_entities: BTreeSet<String>,
    /// Margins home that selects and authorizes the catalyst generator.
    pub generator_home: PathBuf,
}

pub(crate) struct Provisioned {
    pub index: SearchIndex,
    /// Entities whose catalysts a hosted generator (re)wrote in this run.
    pub hosted_generation_calls: usize,
    /// Whether this run forced every document through indexing.
    pub full_reindex: bool,
}

struct Resolved {
    config: EnzymeConfig,
    /// Indexing root: a lone Markdown source keeps the engine's path-addressed
    /// identity; every other Workspace indexes from the empty state directory.
    vault_path: PathBuf,
    /// The lone Markdown source name in path-addressed mode.
    lone_markdown: Option<String>,
}

fn resolve(workspace: &EngineWorkspace) -> Result<Resolved> {
    let parsed = enzyme_spec::parse(&workspace.program_text)
        .context("parsing the lowered Workspace program")?;
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let program = enzyme_spec::resolve(vec![parsed], &home)
        .context("resolving the lowered Workspace program")?;
    let declared = program
        .workspaces
        .iter()
        .find(|candidate| candidate.name == workspace.workspace)
        .with_context(|| format!("program declares no workspace {:?}", workspace.workspace))?;
    let (vault_path, lone_markdown) = match (declared.markdown_path(), declared.markdown_sources().next()) {
        (Some(path), Some(source)) => (PathBuf::from(path), Some(source.name.clone())),
        _ => (workspace.state_dir.clone(), None),
    };
    let config = EnzymeConfig::from_program_workspace(program, &workspace.workspace)?;
    Ok(Resolved {
        config,
        vault_path,
        lone_markdown,
    })
}

/// Every Markdown document the engine would index for this Workspace, with
/// the engine's own identity rules and exclusions.
pub(crate) fn markdown_documents(workspace: &EngineWorkspace) -> Result<Vec<MarkdownDocument>> {
    let resolved = resolve(workspace)?;
    let discovery = if resolved.config.notes_roots.is_empty() {
        if resolved.lone_markdown.is_none() {
            return Ok(Vec::new());
        }
        FileDiscovery::new(resolved.vault_path.clone())
    } else {
        FileDiscovery::from_notes_roots(resolved.config.notes_roots.clone())
    }
    .with_excluded_folders(resolved.config.excluded_folders.clone());
    Ok(discovery
        .discover_named()?
        .into_iter()
        .map(|file| MarkdownDocument {
            source: resolved
                .lone_markdown
                .clone()
                .unwrap_or_else(|| file.root_name.clone()),
            path: file.root_path.join(&file.relative_path),
            source_ref: file.source_ref,
        })
        .collect())
}

/// The curated catalyst entities the engine will select for, as
/// `(lowercased name, entity type)` pairs.
pub(crate) fn curated_entities(workspace: &EngineWorkspace) -> Result<Vec<(String, String)>> {
    Ok(resolve(workspace)?
        .config
        .parse_preferences()
        .entities
        .into_iter()
        .map(|entity| {
            (
                entity.name.trim().to_ascii_lowercase(),
                entity.entity_type.as_str().to_string(),
            )
        })
        .collect())
}

/// Index, embed, select, and generate catalysts for the Workspace.
pub(crate) fn provision(
    workspace: &EngineWorkspace,
    request: ProvisionRequest,
) -> Result<Provisioned> {
    let resolved = resolve(workspace)?;
    let db_path = workspace.db_path();
    let marker = workspace.state_dir.join(IDENTITY_MARKER);
    let current_identity = std::fs::read_to_string(&marker)
        .ok()
        .is_some_and(|value| value.trim() == DOCUMENT_IDENTITY);
    let full_reindex = !db_path.exists() || !current_identity;
    if db_path.is_file() {
        let database = recall_engine::db::Database::open(&db_path)?;
        if !current_identity {
            drop_identity_bearing_catalysts(&database)?;
        }
        reconcile_link_catalysts(
            &database,
            &request.selected_link_entities,
            &request.excluded_link_entities,
        )?;
    }
    if full_reindex {
        debug(&format!(
            "workspace index full_reindex=true reason={}",
            if db_path.exists() { "document_identity" } else { "first_build" }
        ));
    }
    let catalysts_before = catalyst_ids_at(&db_path)?;
    let generator = resolve_generator(&request.generator_home)?;
    let hosted_generator = !matches!(generator.provider(), ConfigProvider::Local);
    let options = SearchOptions {
        workspace_config: Some(resolved.config),
        full_reindex,
        ..SearchOptions::new(&resolved.vault_path, &db_path)
            .with_max_bridged_entities(request.max_bridged_entities)
    };
    let index = ensure_searchable_sync(options, Some(generator))
        .context("indexing and generating recall catalysts")?;
    if !current_identity {
        std::fs::write(&marker, format!("{DOCUMENT_IDENTITY}\n"))
            .with_context(|| format!("writing {}", marker.display()))?;
    }
    let catalysts_after = catalyst_ids(index.database())?;
    let hosted_generation_calls = if hosted_generator {
        catalysts_after
            .iter()
            .filter(|(entity, ids)| catalysts_before.get(*entity) != Some(*ids))
            .count()
    } else {
        0
    };
    Ok(Provisioned {
        index,
        hosted_generation_calls,
        full_reindex,
    })
}

/// Open an existing index read-only. `None` when it was never built.
pub(crate) fn open(db_path: &Path) -> Result<Option<SearchIndex>> {
    // The vault path only seeds the embedding service; it is never read.
    recall_engine::kernel::open_existing_sync(db_path.parent().unwrap_or(db_path), db_path)
}

/// Document count of an existing index, opened read-only after checking it
/// matches the schema this binary expects. Never migrates.
pub(crate) fn indexed_document_count(db_path: &Path) -> Result<usize> {
    let database = recall_engine::db::Database::open_read_only(db_path)?;
    database.validate_expected_schema()?;
    Ok(database.get_document_count()? as usize)
}

/// Whether the index at `state_dir` was built with this binary's identity.
pub(crate) fn has_current_identity(state_dir: &Path) -> bool {
    std::fs::read_to_string(state_dir.join(IDENTITY_MARKER))
        .is_ok_and(|value| value.trim() == DOCUMENT_IDENTITY)
}

/// Folder and collection catalysts are keyed by document identity (hashed
/// root namespaces in earlier releases); they cannot be reselected after the
/// identity changes and would otherwise linger as orphans.
fn drop_identity_bearing_catalysts(database: &recall_engine::db::Database) -> Result<()> {
    let stale = database.query_read(
        "SELECT DISTINCT entity, json_extract(metadata, '$.entity_type') FROM catalysts
         WHERE json_extract(metadata, '$.entity_type') IN ('folder', 'collection')",
        Vec::new(),
        |row| Ok((row.get::<String>(0)?, row.get::<String>(1)?)),
    )?;
    for (entity, kind) in stale {
        database.delete_catalysts_for_entity(&entity, &kind)?;
        database.delete_catalyst_entity_hash(&entity, &kind)?;
    }
    Ok(())
}

pub(crate) fn reconcile_link_catalysts(
    database: &recall_engine::db::Database,
    selected: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> Result<()> {
    let existing_links = database.query_read(
        "SELECT DISTINCT entity FROM catalysts
         WHERE json_extract(metadata, '$.entity_type') = 'link'",
        Vec::new(),
        |row| row.get::<String>(0),
    )?;
    for entity in existing_links
        .into_iter()
        .filter(|entity| !selected.contains(entity))
        .chain(excluded.iter().cloned())
        .collect::<BTreeSet<_>>()
    {
        database.delete_catalysts_for_entity(&entity, "link")?;
        database.delete_catalyst_entity_hash(&entity, "link")?;
    }
    Ok(())
}

fn catalyst_ids_at(db_path: &Path) -> Result<BTreeMap<String, BTreeSet<String>>> {
    if !db_path.is_file() {
        return Ok(BTreeMap::new());
    }
    catalyst_ids(&recall_engine::db::Database::open(db_path)?)
}

fn catalyst_ids(
    database: &recall_engine::db::Database,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let mut result = BTreeMap::new();
    for catalyst in database.get_all_catalysts()? {
        result
            .entry(catalyst.entity)
            .or_insert_with(BTreeSet::new)
            .insert(catalyst.id);
    }
    Ok(result)
}

static GENERATOR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Resolve one generator through Enzyme's embedder-owned-home policy. Resolution
/// owns its Tokio runtime on a worker so the synchronous CLI, desktop blocking
/// lane, and server lane do not inherit one another's runtime requirements.
/// Any failure is terminal for product indexing; there is no generator-free retry.
pub(crate) fn resolve_generator(home: &Path) -> Result<Generator> {
    crate::recall::ensure_usable_generator_at(home)?;
    let status = margins_workflows::catalyst::selected_status(home);
    let hosted_bundle = (status.mode == margins_workflows::catalyst::CatalystMode::Hosted)
        .then(|| crate::hosted_credentials::cached_bundle_for_generation(home))
        .transpose()?
        .flatten();
    let home = home.to_path_buf();
    let result = std::thread::spawn(move || {
        struct RestoreGeneratorEnv {
            values: Vec<(&'static str, Option<std::ffi::OsString>)>,
        }
        impl Drop for RestoreGeneratorEnv {
            fn drop(&mut self) {
                for (name, value) in self.values.drain(..) {
                    match value {
                        Some(value) => std::env::set_var(name, value),
                        None => std::env::remove_var(name),
                    }
                }
            }
        }
        let guard = GENERATOR_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let names = [
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_MODEL",
            "OPENROUTER_API_KEY",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_MODEL",
        ];
        let previous = names
            .into_iter()
            .map(|name| (name, std::env::var_os(name)))
            .collect::<Vec<_>>();
        for name in names {
            std::env::remove_var(name);
        }
        if let Some(bundle) = hosted_bundle.as_ref() {
            // The engine reads one OpenAI-compatible environment triple.
            std::env::set_var("OPENAI_API_KEY", &bundle.api_key);
            std::env::set_var("OPENAI_BASE_URL", &bundle.base_url);
            std::env::set_var("OPENAI_MODEL", &bundle.model);
        }
        let _restore_env = RestoreGeneratorEnv { values: previous };
        let _env_guard = guard;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("starting recall credential resolver runtime")?;
        runtime.block_on(Generator::resolve_in(&home))
    })
    .join();

    match result {
        Ok(result) => result.context("resolving configured recall generator"),
        Err(_) => anyhow::bail!("recall generator resolution worker panicked"),
    }
}

fn debug(message: &str) {
    if std::env::var_os("MARGINS_RECALL_DEBUG").is_some() {
        eprintln!("recall: {message}");
    }
}
