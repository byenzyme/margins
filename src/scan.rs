use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use chrono::{TimeZone, Utc};
use serde::Serialize;

use margins_workflows::workspace::{ResolvedWorkspace, WorkspaceEntity, WorkspacePolicy};
use recall_engine::document::{
    most_recent_entry, strip_frontmatter, DocumentProcessor, FileDiscovery, ProcessedDocument,
};
use recall_engine::llm::CATALYST_PROFILES;
use recall_engine::models::EntityType;
use recall_engine::models::ROOT_FOLDER_ENTITY;
use recall_engine::pipeline::entity_map::{
    coverage_recency_score, coverage_recency_weight, is_automatic_coverage_candidate,
    is_recent_coverage_occurrence, select_coverage_candidate_indices, CoverageCandidate,
};
use recall_engine::pipeline::exclusions::FolderExclusions;
use recall_engine::pipeline::selection_steps::{EXPANDABLE_DENSITY_THRESHOLD, MAX_FOLDER_CHILDREN};

/// Filesystem-only evidence scan used before init.
///
/// The full structured result is the setup agent's authoritative evidence
/// substrate. Scan never mutates Workspace configuration or recall state.
pub fn run_scan(workspace: &ResolvedWorkspace) -> Result<()> {
    let suggestion = execute_scan(workspace)?;
    println!("{}", serde_json::to_string(&suggestion)?);
    Ok(())
}

pub(crate) fn evidence_for_compile(workspace: &ResolvedWorkspace) -> Result<serde_json::Value> {
    Ok(serde_json::to_value(execute_scan(workspace)?)?)
}

fn execute_scan(workspace: &ResolvedWorkspace) -> Result<ScanSuggestion> {
    let docs = process_docs(
        &workspace.home_dir,
        &workspace.config.policy.excluded_folders,
    )?;
    let suggestion = build_scan_suggestion(
        &workspace.home_dir,
        &workspace.config_path,
        &workspace.config.policy,
        &docs,
    )?;
    Ok(suggestion)
}

#[derive(Debug, Serialize)]
struct ScanSuggestion {
    /// Existing flat fields are intentionally kept for older setup clients.
    files: usize,
    entities: Vec<String>,
    excluded_folders: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config_path: Option<String>,
    status: ScanStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,

    /// Rich bounded evidence for newer setup agents.
    schema_version: &'static str,
    scan_id: String,
    vault_path: String,
    generated_at: String,
    instructions: Vec<String>,
    summary: ScanSummary,
    entity_curation_candidates: Vec<EntityCurationCandidate>,
    coverage_entities: Vec<EntityEvidence>,
    top_entities: Vec<EntityEvidence>,
    top_tags: Vec<EntityEvidence>,
    top_links: Vec<EntityEvidence>,
    top_folders: Vec<EntityEvidence>,
    folder_stats: Vec<FolderStat>,
    folder_page_entities: Vec<FolderPageEntityGroup>,
    folder_children: Vec<ChildGroup>,
    tag_children: Vec<ChildGroup>,
    frontmatter_samples: Vec<FrontmatterSample>,
    entity_samples: Vec<EntitySample>,
    sample_files: Vec<SampleFile>,
    current_config: CurrentConfigSummary,
    available_profiles: Vec<ProfileSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ScanStatus {
    Suggested,
}

#[derive(Debug, Serialize)]
struct ScanSummary {
    file_count: usize,
    total_words: i64,
    total_chars: i64,
    folder_count: usize,
    tag_count: usize,
    link_count: usize,
    frontmatter_file_count: usize,
    newest_modified: Option<String>,
    oldest_modified: Option<String>,
    flat_vault: bool,
}

#[derive(Debug, Serialize, Clone)]
struct EntityEvidence {
    spec: String,
    entity_type: String,
    name: String,
    frequency: usize,
    file_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_modified: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
struct EntityCurationCandidate {
    #[serde(flatten)]
    evidence: EntityEvidence,
    coverage_selected: bool,
    page_entities: Vec<String>,
    expansion: FolderExpansionEvidence,
    /// Compatibility summary for older setup agents. New clients should prefer
    /// `expansion.mode` and `expansion.expands_automatically`.
    expands_automatically: bool,
    representative_samples: Vec<FileExcerpt>,
}

#[derive(Debug, Serialize, Clone)]
struct FolderExpansionEvidence {
    mode: FolderExpansionMode,
    expands_automatically: bool,
    explicit_expandable_available: bool,
    linked_child_count: usize,
    active_linked_child_count: usize,
    file_count: usize,
    active_link_density: f64,
    automatic_density_threshold: f64,
    reason: String,
    representative_page_entities: Vec<FolderPageEntity>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FolderExpansionMode {
    Automatic,
    ExplicitAvailable,
    NotApplicable,
}

#[derive(Debug, Serialize)]
struct FolderStat {
    folder: String,
    spec: String,
    file_count: usize,
    word_count: i64,
    latest_modified: Option<String>,
    page_entities: Vec<String>,
    expandable_candidate: bool,
}

#[derive(Debug, Serialize, Clone)]
struct FolderPageEntityGroup {
    folder: String,
    spec: String,
    file_count: usize,
    linked_child_count: usize,
    active_linked_child_count: usize,
    active_link_density: f64,
    page_entities: Vec<FolderPageEntity>,
    expandable_candidate: bool,
}

#[derive(Debug, Serialize, Clone)]
struct FolderPageEntity {
    name: String,
    spec: String,
    source_ref: String,
    link_frequency: usize,
    latest_modified: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChildGroup {
    parent: String,
    children: Vec<NamedCount>,
}

#[derive(Debug, Serialize)]
struct NamedCount {
    name: String,
    count: usize,
}

#[derive(Debug, Serialize)]
struct FrontmatterSample {
    source_ref: String,
    title: Option<String>,
    tags: Vec<String>,
    aliases: Vec<String>,
    links: Vec<String>,
    extra_keys: Vec<String>,
    created: Option<String>,
    modified: Option<String>,
}

#[derive(Debug, Serialize)]
struct EntitySample {
    spec: String,
    entity_type: String,
    name: String,
    samples: Vec<FileExcerpt>,
}

#[derive(Debug, Serialize, Clone)]
struct FileExcerpt {
    source_ref: String,
    title: Option<String>,
    modified_at: Option<String>,
    excerpt: String,
}

#[derive(Debug, Serialize)]
struct SampleFile {
    source_ref: String,
    title: Option<String>,
    folders: Vec<String>,
    tags: Vec<String>,
    links: Vec<String>,
    word_count: i64,
    modified_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct CurrentConfigSummary {
    status: String,
    config_path: Option<String>,
    has_curated_entities: bool,
    entities: Vec<WorkspaceEntity>,
    excluded_folders: Vec<String>,
    excluded_tags: Vec<String>,
    excluded_links: Vec<String>,
    excluded_entities: Vec<String>,
    catalyst_format: Option<String>,
    max_embedding_files: Option<usize>,
}

#[derive(Debug, Serialize)]
struct ProfileSummary {
    key: String,
    display_name: String,
    description: String,
}

#[derive(Debug, Default)]
struct Metric {
    frequency: usize,
    frequency_recent: i64,
    weighted_recency_sum: f64,
    files: HashSet<String>,
    latest_modified: Option<i64>,
}

fn process_docs(vault_path: &Path, excluded_folders: &[String]) -> Result<Vec<ProcessedDocument>> {
    let discovery = FileDiscovery::new(vault_path.to_path_buf())
        .with_excluded_folders(excluded_folders.iter().map(String::as_str));
    let paths = discovery.discover()?;
    let processor = DocumentProcessor::new(vault_path.to_path_buf());
    Ok(processor
        .process_all(&paths)
        .into_iter()
        .flatten()
        .collect())
}

fn build_scan_suggestion(
    vault_path: &Path,
    config_path: &Path,
    policy: &WorkspacePolicy,
    docs: &[ProcessedDocument],
) -> Result<ScanSuggestion> {
    let mut folder_counts: HashMap<String, usize> = HashMap::new();
    let mut tag_counts: HashMap<String, usize> = HashMap::new();
    let mut link_counts: HashMap<String, usize> = HashMap::new();

    for doc in docs {
        // Preserve the historic suggestion behavior: root files do not make
        // `folder:.` a candidate unless the whole vault is flat.
        for folder in &doc.folders {
            *folder_counts.entry(folder.clone()).or_default() += 1;
        }
        for tag in &doc.tags {
            *tag_counts.entry(tag.clone()).or_default() += 1;
        }
        for link in &doc.links {
            *link_counts.entry(link.clone()).or_default() += 1;
        }
    }

    let all_folder_page_entities = folder_page_entities(
        docs,
        &folder_counts,
        &link_counts,
        usize::MAX,
        MAX_FOLDER_CHILDREN,
    );
    let entities = suggested_entities(
        docs,
        &folder_counts,
        &tag_counts,
        &link_counts,
        &all_folder_page_entities,
    );
    let metrics = build_metrics(docs);
    let coverage_specs = coverage_candidate_specs(&metrics, policy);
    let coverage_entities =
        select_coverage_entities(&coverage_specs, &metrics, &all_folder_page_entities, 6);
    let entity_curation_candidates = entity_curation_candidates(
        docs,
        &metrics,
        &all_folder_page_entities,
        &coverage_entities,
    );
    let folder_page_entities = all_folder_page_entities
        .iter()
        .take(25)
        .cloned()
        .collect::<Vec<_>>();
    let current_config = current_config_summary(config_path, policy);

    Ok(ScanSuggestion {
        files: docs.len(),
        entities: entities.clone(),
        excluded_folders: proposed_excluded_folders(policy, docs),
        config_path: None,
        status: ScanStatus::Suggested,
        message: None,
        schema_version: "scan.v2",
        scan_id: format!("scan-{}", Utc::now().format("%Y%m%dT%H%M%S%.3fZ")),
        vault_path: vault_path.to_string_lossy().to_string(),
        generated_at: Utc::now().to_rfc3339(),
        instructions: vec![
            "Use entities/top_entities as bounded evidence for the setup conversation; keep existing flat fields for compatibility.".to_string(),
            "Review entity_curation_candidates as a bounded, name-neutral evidence surface even when frequency ranking leaves a folder out of the initial evidence set. Infer each folder's role from representative_samples, page_entities, link structure, and the user's vocabulary; the scan does not classify folders or recommend profiles from their names.".to_string(),
            "Use coverage_entities to ground the Workspace review. It borrows Enzyme's greedy marginal-file-coverage posture so distinct lanes survive instead of simply showing the largest two folders.".to_string(),
            "Workspace policy.entities uses the same simple-or-options shape and semantics as Enzyme entities. Non-empty is the exact explicit catalyst surface, not a numeric weight, and Margins appends no automatic entities. Empty means automatic: note-only Workspaces use Enzyme coverage, while connected-ledger Workspaces use noise-filtered correspondents/recent people and fail closed if none are safe.".to_string(),
            "Prefer base folders over descendant folders; inspect folder_children/folder_stats for covered descendants.".to_string(),
            "Use each entity_curation_candidates entry's expansion object before consulting compatibility fields. mode=automatic means expands_automatically=true and config expandable=true would be redundant; mode=explicit_available means real child pages exist below the automatic threshold and expandable=true is available only if the user wants separate threads. Never persist config children.".to_string(),
            "Prefer top_entities with supporting entity_samples; avoid structural folders listed in excluded_folders.".to_string(),
            "Use available_profiles for optional, evidence-backed entity profile overrides; leave ambiguous entities without an override.".to_string(),
            "Use folder/tag/link child maps and frontmatter_samples to ask the user for corrections before applying settings.".to_string(),
        ],
        summary: build_summary(docs, &metrics),
        entity_curation_candidates,
        coverage_entities,
        top_entities: evidence_for_specs(&entities, &metrics),
        top_tags: top_evidence_by_type(&metrics, "tag", 25),
        top_links: top_evidence_by_type(&metrics, "link", 25),
        top_folders: top_evidence_by_type(&metrics, "folder", 25),
        folder_stats: folder_stats(docs, &metrics, &all_folder_page_entities, 50),
        folder_page_entities,
        folder_children: folder_children(&metrics, 25, 25),
        tag_children: tag_children(&metrics, 25, 25),
        frontmatter_samples: frontmatter_samples(docs, 12),
        entity_samples: entity_samples(docs, &entities, 12, 3),
        sample_files: sample_files(docs, 30),
        current_config,
        available_profiles: available_profiles(),
    })
}

fn build_metrics(docs: &[ProcessedDocument]) -> HashMap<String, Metric> {
    let mut metrics = HashMap::new();

    for doc in docs {
        for folder in folder_names_for_doc(doc) {
            if !is_structural_folder(&folder) {
                add_metric(&mut metrics, entity_spec("folder", &folder), doc);
            }
        }
        for tag in &doc.tags {
            if !is_structural_tag(tag) {
                add_metric(&mut metrics, entity_spec("tag", tag), doc);
            }
        }
        for link in &doc.links {
            add_metric(&mut metrics, entity_spec("link", link), doc);
        }
        if doc.is_log && doc.append_log.is_some() {
            if let Some(name) = recall_engine::document::log_entity_name(doc) {
                add_metric(&mut metrics, entity_spec("log", &name), doc);
            }
        }
    }

    metrics
}

fn add_metric(metrics: &mut HashMap<String, Metric>, spec: String, doc: &ProcessedDocument) {
    let metric = metrics.entry(spec).or_default();
    metric.frequency += 1;
    let age_days =
        (Utc::now().timestamp_millis() - doc.created_at) as f64 / (1000.0 * 60.0 * 60.0 * 24.0);
    metric.weighted_recency_sum += coverage_recency_weight(age_days);
    if is_recent_coverage_occurrence(age_days) {
        metric.frequency_recent += 1;
    }
    metric.files.insert(doc.source_ref.clone());
    metric.latest_modified = Some(
        metric
            .latest_modified
            .unwrap_or(i64::MIN)
            .max(doc.modified_at),
    );
}

fn build_summary(docs: &[ProcessedDocument], metrics: &HashMap<String, Metric>) -> ScanSummary {
    let newest_modified = docs.iter().map(|doc| doc.modified_at).max();
    let oldest_modified = docs.iter().map(|doc| doc.modified_at).min();

    ScanSummary {
        file_count: docs.len(),
        total_words: docs.iter().map(|doc| doc.word_count).sum(),
        total_chars: docs.iter().map(|doc| doc.char_count).sum(),
        folder_count: metrics
            .keys()
            .filter(|spec| spec.starts_with("folder:"))
            .count(),
        tag_count: metrics.keys().filter(|spec| spec.starts_with('#')).count(),
        link_count: metrics.keys().filter(|spec| spec.starts_with("[[")).count(),
        frontmatter_file_count: docs.iter().filter(|doc| doc.frontmatter.is_some()).count(),
        newest_modified: newest_modified.and_then(millis_to_rfc3339),
        oldest_modified: oldest_modified.and_then(millis_to_rfc3339),
        flat_vault: is_flat_vault(docs),
    }
}

fn current_config_summary(config_path: &Path, policy: &WorkspacePolicy) -> CurrentConfigSummary {
    CurrentConfigSummary {
        status: "configured".to_string(),
        config_path: Some(config_path.to_string_lossy().to_string()),
        has_curated_entities: !policy.entities.is_empty(),
        entities: policy.entities.clone(),
        excluded_folders: policy.excluded_folders.clone(),
        excluded_tags: policy.excluded_tags.clone(),
        excluded_links: Vec::new(),
        excluded_entities: policy.excluded_entities.clone(),
        catalyst_format: None,
        max_embedding_files: None,
    }
}

fn available_profiles() -> Vec<ProfileSummary> {
    CATALYST_PROFILES
        .iter()
        .map(|profile| ProfileSummary {
            key: profile.key.to_string(),
            display_name: profile.display_name.to_string(),
            description: profile.description.to_string(),
        })
        .collect()
}

fn evidence_for_specs(specs: &[String], metrics: &HashMap<String, Metric>) -> Vec<EntityEvidence> {
    specs
        .iter()
        .filter_map(|spec| entity_ref_from_spec(spec).map(|entity_ref| (spec, entity_ref)))
        .map(|(spec, entity_ref)| evidence_from_metric(spec, entity_ref, metrics.get(spec)))
        .collect()
}

fn top_evidence_by_type(
    metrics: &HashMap<String, Metric>,
    entity_type: &str,
    limit: usize,
) -> Vec<EntityEvidence> {
    let mut entries: Vec<_> = metrics
        .iter()
        .filter_map(|(spec, metric)| {
            let entity_ref = entity_ref_from_spec(spec)?;
            (entity_ref.entity_type == entity_type)
                .then(|| evidence_from_metric(spec, entity_ref, Some(metric)))
        })
        .collect();

    sort_evidence(&mut entries);
    entries.truncate(limit);
    entries
}

fn entity_curation_candidates(
    docs: &[ProcessedDocument],
    metrics: &HashMap<String, Metric>,
    folder_page_entities: &[FolderPageEntityGroup],
    coverage_entities: &[EntityEvidence],
) -> Vec<EntityCurationCandidate> {
    let coverage_specs = coverage_entities
        .iter()
        .map(|entity| entity.spec.as_str())
        .collect::<HashSet<_>>();
    let page_entities_by_folder = folder_page_entities
        .iter()
        .map(|group| (group.folder.as_str(), group))
        .collect::<HashMap<_, _>>();
    let mut candidates = metrics
        .iter()
        .filter_map(|(spec, metric)| {
            let entity_ref = entity_ref_from_spec(spec)?;
            if entity_ref.entity_type != "folder" || entity_ref.name == ROOT_FOLDER_ENTITY {
                return None;
            }
            let page_group = page_entities_by_folder.get(entity_ref.name).copied();
            let expansion = folder_expansion_evidence(page_group, metric.files.len());
            Some(EntityCurationCandidate {
                evidence: evidence_from_metric(spec, entity_ref, Some(metric)),
                coverage_selected: coverage_specs.contains(spec.as_str()),
                page_entities: page_group
                    .map(|group| {
                        group
                            .page_entities
                            .iter()
                            .map(|entity| entity.name.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
                expands_automatically: page_group
                    .map(|group| group.expandable_candidate)
                    .unwrap_or(false),
                expansion,
                representative_samples: Vec::new(),
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.coverage_selected
            .cmp(&a.coverage_selected)
            .then_with(|| (!b.page_entities.is_empty()).cmp(&(!a.page_entities.is_empty())))
            .then_with(|| b.evidence.file_count.cmp(&a.evidence.file_count))
            .then_with(|| a.evidence.spec.cmp(&b.evidence.spec))
    });
    candidates.truncate(50);
    for candidate in &mut candidates {
        if let Some(entity_ref) = entity_ref_from_spec(&candidate.evidence.spec) {
            candidate.representative_samples = representative_entity_samples(docs, entity_ref, 2);
        }
    }
    candidates
}

fn folder_expansion_evidence(
    page_group: Option<&FolderPageEntityGroup>,
    file_count: usize,
) -> FolderExpansionEvidence {
    let Some(group) = page_group else {
        return FolderExpansionEvidence {
            mode: FolderExpansionMode::NotApplicable,
            expands_automatically: false,
            explicit_expandable_available: false,
            linked_child_count: 0,
            active_linked_child_count: 0,
            file_count,
            active_link_density: 0.0,
            automatic_density_threshold: EXPANDABLE_DENSITY_THRESHOLD,
            reason: "no_linked_child_pages".to_string(),
            representative_page_entities: Vec::new(),
        };
    };

    let mode = if group.expandable_candidate {
        FolderExpansionMode::Automatic
    } else {
        FolderExpansionMode::ExplicitAvailable
    };
    let reason = match mode {
        FolderExpansionMode::Automatic => "active_child_density_meets_automatic_threshold",
        FolderExpansionMode::ExplicitAvailable => "linked_child_pages_below_automatic_threshold",
        FolderExpansionMode::NotApplicable => "no_linked_child_pages",
    }
    .to_string();

    FolderExpansionEvidence {
        explicit_expandable_available: mode == FolderExpansionMode::ExplicitAvailable,
        expands_automatically: mode == FolderExpansionMode::Automatic,
        mode,
        linked_child_count: group.linked_child_count,
        active_linked_child_count: group.active_linked_child_count,
        file_count: group.file_count,
        active_link_density: group.active_link_density,
        automatic_density_threshold: EXPANDABLE_DENSITY_THRESHOLD,
        reason,
        representative_page_entities: group.page_entities.iter().take(5).cloned().collect(),
    }
}

fn select_coverage_entities(
    specs: &[String],
    metrics: &HashMap<String, Metric>,
    folder_page_entities: &[FolderPageEntityGroup],
    limit: usize,
) -> Vec<EntityEvidence> {
    let active_folder_links = folder_page_entities
        .iter()
        .map(|group| {
            let active = group
                .page_entities
                .iter()
                .filter(|entity| {
                    metrics
                        .get(&entity.spec)
                        .is_some_and(|metric| metric.frequency_recent > 0)
                })
                .count();
            (group.folder.as_str(), active)
        })
        .collect::<HashMap<_, _>>();
    let candidates = specs
        .iter()
        .filter_map(|spec| {
            let entity_ref = entity_ref_from_spec(spec)?;
            let metric = metrics.get(spec)?;
            let entity_type = match entity_ref.entity_type {
                "folder" => EntityType::Folder,
                "tag" => EntityType::Tag,
                "link" => EntityType::Link,
                "log" => EntityType::AppendLog,
                _ => return None,
            };
            let ranking_weight = metric.frequency as f64;
            Some((
                evidence_from_metric(spec, entity_ref, Some(metric)),
                CoverageCandidate {
                    entity_type,
                    ranking_weight,
                    recency_score: coverage_recency_score(
                        metric.weighted_recency_sum,
                        ranking_weight,
                    ),
                    frequency_recent: metric.frequency_recent,
                    files: metric.files.clone(),
                    active_folder_links: active_folder_links
                        .get(entity_ref.name)
                        .copied()
                        .unwrap_or(0),
                },
            ))
        })
        .collect::<Vec<_>>();
    let scoring_inputs = candidates
        .iter()
        .map(|(_, candidate)| candidate.clone())
        .collect::<Vec<_>>();

    select_coverage_candidate_indices(&scoring_inputs, limit)
        .into_iter()
        .map(|index| candidates[index].0.clone())
        .collect()
}

fn coverage_candidate_specs(
    metrics: &HashMap<String, Metric>,
    policy: &WorkspacePolicy,
) -> Vec<String> {
    let folder_exclusions =
        FolderExclusions::with_extra(policy.excluded_folders.iter().map(String::as_str));
    let has_non_root_folder = metrics.keys().any(|spec| {
        spec.strip_prefix("folder:")
            .is_some_and(|folder| folder != ROOT_FOLDER_ENTITY)
    });
    let mut specs = metrics
        .keys()
        .filter_map(|spec| {
            let entity = entity_ref_from_spec(spec)?;
            let entity_type = match entity.entity_type {
                "folder" => EntityType::Folder,
                "tag" => EntityType::Tag,
                "link" => EntityType::Link,
                "log" => EntityType::AppendLog,
                _ => return None,
            };
            let ranking_weight = metrics.get(spec)?.frequency as f64;
            let eligible = (entity_type == EntityType::Folder
                && entity.name == ROOT_FOLDER_ENTITY
                && !has_non_root_folder)
                || is_automatic_coverage_candidate(
                    &entity_type,
                    entity.name,
                    ranking_weight,
                    &folder_exclusions,
                );
            eligible.then(|| spec.clone())
        })
        .collect::<Vec<_>>();
    specs.sort();
    specs
}

fn evidence_from_metric(
    spec: &str,
    entity_ref: EntityRef,
    metric: Option<&Metric>,
) -> EntityEvidence {
    EntityEvidence {
        spec: spec.to_string(),
        entity_type: entity_ref.entity_type.to_string(),
        name: entity_ref.name.to_string(),
        frequency: metric.map(|m| m.frequency).unwrap_or(0),
        file_count: metric.map(|m| m.files.len()).unwrap_or(0),
        latest_modified: metric
            .and_then(|m| m.latest_modified)
            .and_then(millis_to_rfc3339),
    }
}

fn sort_evidence(entries: &mut [EntityEvidence]) {
    entries.sort_by(|a, b| {
        b.file_count
            .cmp(&a.file_count)
            .then_with(|| b.frequency.cmp(&a.frequency))
            .then_with(|| a.spec.cmp(&b.spec))
    });
}

fn folder_page_entities(
    docs: &[ProcessedDocument],
    folder_counts: &HashMap<String, usize>,
    link_counts: &HashMap<String, usize>,
    parent_limit: usize,
    child_limit: usize,
) -> Vec<FolderPageEntityGroup> {
    let mut direct_pages: HashMap<String, Vec<FolderPageEntity>> = HashMap::new();

    for doc in docs {
        let Some((folder, stem)) = direct_child_stem(&doc.source_ref) else {
            continue;
        };
        let folder = folder.to_lowercase();
        if is_structural_folder(&folder) || !folder_counts.contains_key(&folder) {
            continue;
        }

        let link_name = stem.to_lowercase();
        let Some(link_frequency) = link_counts.get(&link_name).copied() else {
            continue;
        };

        direct_pages
            .entry(folder)
            .or_default()
            .push(FolderPageEntity {
                name: link_name.clone(),
                spec: entity_spec("link", &link_name),
                source_ref: doc.source_ref.clone(),
                link_frequency,
                latest_modified: millis_to_rfc3339(doc.modified_at),
            });
    }

    let mut groups: Vec<_> = direct_pages
        .into_iter()
        .filter_map(|(folder, mut page_entities)| {
            page_entities.sort_by(|a, b| {
                b.link_frequency
                    .cmp(&a.link_frequency)
                    .then_with(|| b.latest_modified.cmp(&a.latest_modified))
                    .then_with(|| a.name.cmp(&b.name))
            });
            page_entities.dedup_by(|a, b| a.name == b.name);

            let file_count = folder_counts.get(&folder).copied().unwrap_or_default();
            if file_count == 0 {
                return None;
            }

            let active_page_entity_count = page_entities
                .iter()
                .filter(|entity| entity.link_frequency >= 3)
                .count();
            let linked_child_count = page_entities.len();
            let expandable_candidate =
                active_page_entity_count as f64 / file_count as f64 >= EXPANDABLE_DENSITY_THRESHOLD;
            let active_link_density = active_page_entity_count as f64 / file_count as f64;

            page_entities.truncate(child_limit);
            Some(FolderPageEntityGroup {
                spec: entity_spec("folder", &folder),
                folder,
                file_count,
                linked_child_count,
                active_linked_child_count: active_page_entity_count,
                active_link_density,
                page_entities,
                expandable_candidate,
            })
        })
        .collect();

    groups.sort_by(|a, b| {
        b.expandable_candidate
            .cmp(&a.expandable_candidate)
            .then_with(|| b.page_entities.len().cmp(&a.page_entities.len()))
            .then_with(|| b.file_count.cmp(&a.file_count))
            .then_with(|| a.folder.cmp(&b.folder))
    });
    groups.truncate(parent_limit);
    groups
}

fn direct_child_stem(source_ref: &str) -> Option<(&str, &str)> {
    let (folder, file_name) = source_ref.split_once('/')?;
    if folder.is_empty() || file_name.contains('/') {
        return None;
    }
    let stem = file_name.strip_suffix(".md").unwrap_or(file_name).trim();
    (!stem.is_empty()).then_some((folder, stem))
}

fn folder_stats(
    docs: &[ProcessedDocument],
    metrics: &HashMap<String, Metric>,
    folder_page_entities: &[FolderPageEntityGroup],
    limit: usize,
) -> Vec<FolderStat> {
    let mut word_counts: HashMap<String, i64> = HashMap::new();
    for doc in docs {
        for folder in folder_names_for_doc(doc) {
            if !is_structural_folder(&folder) {
                *word_counts.entry(folder).or_default() += doc.word_count;
            }
        }
    }

    let page_entity_by_folder: HashMap<&str, &FolderPageEntityGroup> = folder_page_entities
        .iter()
        .map(|group| (group.folder.as_str(), group))
        .collect();

    let mut stats: Vec<_> = metrics
        .iter()
        .filter_map(|(spec, metric)| {
            let entity_ref = entity_ref_from_spec(spec)?;
            (entity_ref.entity_type == "folder").then(|| {
                let page_group = page_entity_by_folder.get(entity_ref.name).copied();
                FolderStat {
                    folder: entity_ref.name.to_string(),
                    spec: spec.clone(),
                    file_count: metric.files.len(),
                    word_count: *word_counts.get(entity_ref.name).unwrap_or(&0),
                    latest_modified: metric.latest_modified.and_then(millis_to_rfc3339),
                    page_entities: page_group
                        .map(|group| {
                            group
                                .page_entities
                                .iter()
                                .map(|entity| entity.name.clone())
                                .collect()
                        })
                        .unwrap_or_default(),
                    expandable_candidate: page_group
                        .map(|group| group.expandable_candidate)
                        .unwrap_or(false),
                }
            })
        })
        .collect();
    stats.sort_by(|a, b| {
        b.file_count
            .cmp(&a.file_count)
            .then_with(|| a.folder.cmp(&b.folder))
    });
    stats.truncate(limit);
    stats
}

fn folder_children(
    metrics: &HashMap<String, Metric>,
    parent_limit: usize,
    child_limit: usize,
) -> Vec<ChildGroup> {
    let mut groups: HashMap<String, Vec<NamedCount>> = HashMap::new();

    for (spec, metric) in metrics {
        let Some(entity_ref) = entity_ref_from_spec(spec) else {
            continue;
        };
        if entity_ref.entity_type != "folder" || entity_ref.name == ROOT_FOLDER_ENTITY {
            continue;
        }
        let parent = entity_ref
            .name
            .rsplit_once('/')
            .map(|(parent, _)| parent.to_string())
            .unwrap_or_else(|| ROOT_FOLDER_ENTITY.to_string());
        groups.entry(parent).or_default().push(NamedCount {
            name: entity_ref.name.to_string(),
            count: metric.files.len(),
        });
    }

    sorted_child_groups(groups, parent_limit, child_limit)
}

fn tag_children(
    metrics: &HashMap<String, Metric>,
    parent_limit: usize,
    child_limit: usize,
) -> Vec<ChildGroup> {
    let mut groups: HashMap<String, Vec<NamedCount>> = HashMap::new();

    for (spec, metric) in metrics {
        let Some(entity_ref) = entity_ref_from_spec(spec) else {
            continue;
        };
        if entity_ref.entity_type != "tag" || !entity_ref.name.contains('/') {
            continue;
        }
        let parent = entity_ref
            .name
            .rsplit_once('/')
            .map(|(parent, _)| parent.to_string())
            .unwrap_or_default();
        groups.entry(parent).or_default().push(NamedCount {
            name: entity_ref.name.to_string(),
            count: metric.files.len(),
        });
    }

    sorted_child_groups(groups, parent_limit, child_limit)
}

fn sorted_child_groups(
    groups: HashMap<String, Vec<NamedCount>>,
    parent_limit: usize,
    child_limit: usize,
) -> Vec<ChildGroup> {
    let mut groups: Vec<_> = groups
        .into_iter()
        .map(|(parent, mut children)| {
            children.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
            children.truncate(child_limit);
            ChildGroup { parent, children }
        })
        .collect();

    groups.sort_by(|a, b| {
        let a_total: usize = a.children.iter().map(|child| child.count).sum();
        let b_total: usize = b.children.iter().map(|child| child.count).sum();
        b_total.cmp(&a_total).then_with(|| a.parent.cmp(&b.parent))
    });
    groups.truncate(parent_limit);
    groups
}

fn frontmatter_samples(docs: &[ProcessedDocument], limit: usize) -> Vec<FrontmatterSample> {
    let mut docs: Vec<_> = docs
        .iter()
        .filter(|doc| doc.frontmatter.is_some() && !is_structural_doc(doc))
        .collect();
    docs.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.source_ref.cmp(&b.source_ref))
    });

    docs.into_iter()
        .take(limit)
        .filter_map(|doc| {
            let fm = doc.frontmatter.as_ref()?;
            let mut extra_keys: Vec<_> = fm.extra.keys().cloned().collect();
            extra_keys.sort();
            Some(FrontmatterSample {
                source_ref: doc.source_ref.clone(),
                title: fm.title.clone().or_else(|| doc.title.clone()),
                tags: fm.tags.clone(),
                aliases: fm.aliases.clone(),
                links: fm.links.clone(),
                extra_keys,
                created: fm.created.map(|dt| dt.to_rfc3339()),
                modified: fm.modified.map(|dt| dt.to_rfc3339()),
            })
        })
        .collect()
}

fn entity_samples(
    docs: &[ProcessedDocument],
    entity_specs: &[String],
    entity_limit: usize,
    samples_per_entity: usize,
) -> Vec<EntitySample> {
    entity_specs
        .iter()
        .take(entity_limit)
        .filter_map(|spec| {
            let entity_ref = entity_ref_from_spec(spec)?;
            let samples = representative_entity_samples(docs, entity_ref, samples_per_entity);

            (!samples.is_empty()).then(|| EntitySample {
                spec: spec.clone(),
                entity_type: entity_ref.entity_type.to_string(),
                name: entity_ref.name.to_string(),
                samples,
            })
        })
        .collect()
}

fn representative_entity_samples(
    docs: &[ProcessedDocument],
    entity_ref: EntityRef<'_>,
    limit: usize,
) -> Vec<FileExcerpt> {
    let mut matching_docs = docs
        .iter()
        .filter(|doc| doc_matches_entity(doc, &entity_ref))
        .collect::<Vec<_>>();
    matching_docs.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.source_ref.cmp(&b.source_ref))
    });
    matching_docs
        .into_iter()
        .take(limit)
        .map(|doc| FileExcerpt {
            source_ref: doc.source_ref.clone(),
            title: doc.title.clone(),
            modified_at: millis_to_rfc3339(doc.modified_at),
            excerpt: excerpt_for_entity(doc, &entity_ref),
        })
        .collect()
}

fn sample_files(docs: &[ProcessedDocument], limit: usize) -> Vec<SampleFile> {
    let mut docs: Vec<_> = docs.iter().filter(|doc| !is_structural_doc(doc)).collect();
    docs.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.source_ref.cmp(&b.source_ref))
    });

    docs.into_iter()
        .take(limit)
        .map(|doc| SampleFile {
            source_ref: doc.source_ref.clone(),
            title: doc.title.clone(),
            folders: folder_names_for_doc(doc),
            tags: doc
                .tags
                .iter()
                .filter(|tag| !is_structural_tag(tag))
                .cloned()
                .collect(),
            links: doc.links.clone(),
            word_count: doc.word_count,
            modified_at: millis_to_rfc3339(doc.modified_at),
        })
        .collect()
}

fn doc_matches_entity(doc: &ProcessedDocument, entity_ref: &EntityRef<'_>) -> bool {
    match entity_ref.entity_type {
        "folder" => folder_names_for_doc(doc)
            .iter()
            .any(|folder| folder == entity_ref.name),
        "tag" => doc.tags.iter().any(|tag| tag == entity_ref.name),
        "link" => doc.links.iter().any(|link| link == entity_ref.name),
        "log" => {
            doc.is_log
                && doc.append_log.is_some()
                && recall_engine::document::log_entity_name(doc)
                    .as_deref()
                    .is_some_and(|name| name == entity_ref.name)
        }
        _ => false,
    }
}

fn excerpt_for_entity(doc: &ProcessedDocument, entity_ref: &EntityRef<'_>) -> String {
    let body = strip_frontmatter(&doc.content);
    if entity_ref.entity_type == "log" {
        if let Some(log) = doc.append_log.as_ref() {
            return clean_excerpt(&most_recent_entry(body, log));
        }
    }

    let needles = match entity_ref.entity_type {
        "tag" => vec![format!("#{}", entity_ref.name), entity_ref.name.to_string()],
        "link" => vec![
            format!("[[{}", entity_ref.name),
            entity_ref.name.to_string(),
        ],
        _ => vec![entity_ref.name.to_string()],
    };

    for needle in needles {
        if let Some(excerpt) = excerpt_around(body, &needle, 320) {
            return excerpt;
        }
    }

    first_excerpt(body, 320)
}

fn excerpt_around(content: &str, needle: &str, max_chars: usize) -> Option<String> {
    let lower_content = content.to_lowercase();
    let lower_needle = needle.to_lowercase();
    let index = lower_content.find(&lower_needle)?;
    let start = content[..index]
        .char_indices()
        .rev()
        .nth(max_chars / 4)
        .map(|(idx, _)| idx)
        .unwrap_or(0);
    let end = content[index..]
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| index + idx)
        .unwrap_or(content.len());
    Some(clean_excerpt(&content[start..end]))
}

fn first_excerpt(content: &str, max_chars: usize) -> String {
    let trimmed = content.trim();
    let end = trimmed
        .char_indices()
        .nth(max_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(trimmed.len());
    clean_excerpt(&trimmed[..end])
}

fn clean_excerpt(excerpt: &str) -> String {
    excerpt
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn folder_names_for_doc(doc: &ProcessedDocument) -> Vec<String> {
    if doc.folders.is_empty() {
        vec![ROOT_FOLDER_ENTITY.to_string()]
    } else {
        doc.folders.clone()
    }
}

fn is_structural_doc(doc: &ProcessedDocument) -> bool {
    doc.folders
        .iter()
        .any(|folder| is_structural_folder(folder))
}

#[derive(Clone, Copy)]
struct EntityRef<'a> {
    entity_type: &'a str,
    name: &'a str,
}

fn entity_ref_from_spec(spec: &str) -> Option<EntityRef<'_>> {
    if let Some(name) = spec.strip_prefix("folder:") {
        Some(EntityRef {
            entity_type: "folder",
            name,
        })
    } else if let Some(name) = spec.strip_prefix('#') {
        Some(EntityRef {
            entity_type: "tag",
            name,
        })
    } else if spec.starts_with("[[") && spec.ends_with("]]") {
        Some(EntityRef {
            entity_type: "link",
            name: &spec[2..spec.len() - 2],
        })
    } else if let Some(name) = spec.strip_prefix("log:") {
        Some(EntityRef {
            entity_type: "log",
            name,
        })
    } else {
        None
    }
}

fn entity_spec(entity_type: &str, name: &str) -> String {
    match entity_type {
        "folder" => format!("folder:{name}"),
        "tag" => format!("#{name}"),
        "link" => format!("[[{name}]]"),
        "log" => format!("log:{name}"),
        _ => name.to_string(),
    }
}

fn millis_to_rfc3339(millis: i64) -> Option<String> {
    Utc.timestamp_millis_opt(millis)
        .single()
        .map(|dt| dt.to_rfc3339())
}

fn suggested_folders(folder_counts: &HashMap<String, usize>) -> Vec<String> {
    sorted_counts(folder_counts)
        .into_iter()
        .filter(|(folder, _)| is_base_folder_candidate(folder))
        .map(|(folder, _)| folder)
        .collect()
}

fn suggested_entities(
    docs: &[ProcessedDocument],
    folder_counts: &HashMap<String, usize>,
    tag_counts: &HashMap<String, usize>,
    link_counts: &HashMap<String, usize>,
    folder_page_entities: &[FolderPageEntityGroup],
) -> Vec<String> {
    let mut entities = Vec::new();
    let mut seen = HashSet::new();
    let mut selected_folders = HashSet::new();

    if is_flat_vault(docs) {
        push_entity(&mut entities, &mut seen, "folder:.".to_string());
    } else {
        for folder in suggested_folders(folder_counts) {
            selected_folders.insert(folder.clone());
            push_entity(&mut entities, &mut seen, format!("folder:{folder}"));
        }
    }

    let covered_page_links: HashSet<&str> = folder_page_entities
        .iter()
        .filter(|group| selected_folders.contains(&group.folder))
        .flat_map(|group| {
            group
                .page_entities
                .iter()
                .map(|entity| entity.name.as_str())
        })
        .collect();

    for (tag, _) in sorted_counts(tag_counts)
        .into_iter()
        .filter(|(tag, _)| !is_structural_tag(tag))
        .take(20)
    {
        push_entity(&mut entities, &mut seen, format!("#{tag}"));
    }
    for (link, _) in sorted_counts(link_counts)
        .into_iter()
        .filter(|(link, count)| {
            *count >= 3 && !link.contains('/') && !covered_page_links.contains(link.as_str())
        })
        .take(8)
    {
        push_entity(&mut entities, &mut seen, format!("[[{link}]]"));
    }

    // Recognized append-only logs are evidence for the setup explanation, not
    // automatic config. A user may later make one central through the ordinary
    // corrected desired-state path. Sorting keeps the evidence deterministic.
    let mut log_names: Vec<String> = docs
        .iter()
        .filter(|doc| doc.is_log && doc.append_log.is_some())
        .filter_map(recall_engine::document::log_entity_name)
        .collect();
    log_names.sort();
    for name in log_names {
        push_entity(&mut entities, &mut seen, format!("log:{name}"));
    }

    entities
}

fn is_flat_vault(docs: &[ProcessedDocument]) -> bool {
    !docs.is_empty()
        && docs.iter().all(|doc| {
            doc.folders
                .iter()
                .all(|folder| folder == ROOT_FOLDER_ENTITY)
        })
}

fn is_base_folder_candidate(folder: &str) -> bool {
    !folder.contains('/') && !is_structural_folder(folder) && folder != ROOT_FOLDER_ENTITY
}

fn sorted_counts(map: &HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut entries: Vec<_> = map
        .iter()
        .map(|(name, count)| (name.clone(), *count))
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries
}

fn push_entity(entities: &mut Vec<String>, seen: &mut HashSet<String>, entity: String) {
    if seen.insert(entity.clone()) {
        entities.push(entity);
    }
}

fn proposed_excluded_folders(policy: &WorkspacePolicy, docs: &[ProcessedDocument]) -> Vec<String> {
    let mut proposed = policy.excluded_folders.clone();
    let mut seen: HashSet<String> = proposed
        .iter()
        .filter_map(|folder| normalized_exclusion_key(folder))
        .collect();

    for folder in observed_structural_exclusions(docs) {
        let Some(key) = normalized_exclusion_key(&folder) else {
            continue;
        };
        if seen.insert(key) {
            proposed.push(folder);
        }
    }

    proposed
}

fn observed_structural_exclusions(docs: &[ProcessedDocument]) -> Vec<String> {
    let mut folders = HashSet::new();

    for doc in docs {
        for folder in &doc.folders {
            if let Some(exclusion) = structural_exclusion_for_folder(folder) {
                folders.insert(exclusion);
            }
        }
    }

    let mut folders: Vec<_> = folders.into_iter().collect();
    folders.sort();
    folders
}

fn structural_exclusion_for_folder(folder: &str) -> Option<String> {
    let mut prefix = String::new();

    for component in folder.split('/') {
        let component = component.trim();
        if component.is_empty() {
            continue;
        }
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(component);
        if is_structural_folder_component(component) {
            return Some(prefix);
        }
    }

    None
}

fn normalized_exclusion_key(folder: &str) -> Option<String> {
    let trimmed = folder.trim().trim_matches('/');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn is_structural_folder(folder: &str) -> bool {
    folder.split('/').any(is_structural_folder_component)
}

fn is_structural_folder_component(name: &str) -> bool {
    if name.starts_with('.') && name != ROOT_FOLDER_ENTITY {
        return true;
    }

    let normalized = name.to_ascii_lowercase();
    if normalized == "excalidraw"
        || (normalized.starts_with("qwen-enzyme-") && normalized.ends_with("-benchmark"))
    {
        return true;
    }

    matches!(
        name,
        ".agents"
            | ".aside"
            | ".claude"
            | ".codex"
            | ".codex-work"
            | ".conversations"
            | ".enzyme"
            | ".git"
            | ".hermes"
            | ".local"
            | ".margins"
            | ".obsidian"
            | ".pi"
            | ".trash"
            | "_margins"
            | "__pycache__"
            | "build"
            | "demo"
            | "demo-bible-study-vault"
            | "dist"
            | "node_modules"
            | "obsidian-demo-vault"
            | "target"
            | "templates"
            | "templater"
    )
}

fn is_structural_tag(tag: &str) -> bool {
    matches!(tag, "todo" | "template" | "archived" | "archive")
}

#[cfg(test)]
mod tests {
    use super::*;
    use recall_engine::document::Frontmatter;
    use recall_engine::models::DateSource;
    use tempfile::TempDir;

    fn doc(folders: Vec<&str>) -> ProcessedDocument {
        doc_with(folders, Vec::new(), Vec::new(), None, "note.md", 0, "")
    }

    fn doc_with(
        folders: Vec<&str>,
        tags: Vec<&str>,
        links: Vec<&str>,
        frontmatter: Option<Frontmatter>,
        source_ref: &str,
        modified_at: i64,
        content: &str,
    ) -> ProcessedDocument {
        ProcessedDocument {
            source_ref: source_ref.to_string(),
            title: Some(source_ref.trim_end_matches(".md").to_string()),
            content: content.to_string(),
            content_hash: "hash".to_string(),
            word_count: content.split_whitespace().count() as i64,
            char_count: content.chars().count() as i64,
            created_at: modified_at,
            created_source: DateSource::Filesystem,
            modified_at,
            modified_source: DateSource::Filesystem,
            tags: tags.into_iter().map(str::to_string).collect(),
            links: links.into_iter().map(str::to_string).collect(),
            folders: folders.into_iter().map(str::to_string).collect(),
            chunks: Vec::new(),
            chunk_entry_dates: Vec::new(),
            frontmatter,
            is_log: false,
            append_log: None,
        }
    }

    fn restore_env(old: Option<String>) {
        // SAFETY: tests that mutate MARGINS_HOME hold ENV_LOCK.
        unsafe {
            match old {
                Some(value) => std::env::set_var("MARGINS_HOME", value),
                None => std::env::remove_var("MARGINS_HOME"),
            }
        }
    }

    fn broad_historic_exclusions() -> Vec<String> {
        vec![
            ".agents".to_string(),
            ".aside".to_string(),
            ".claude".to_string(),
            ".codex".to_string(),
            ".codex-work".to_string(),
            ".conversations".to_string(),
            ".enzyme".to_string(),
            ".git".to_string(),
            ".hermes".to_string(),
            ".local".to_string(),
            ".margins".to_string(),
            ".obsidian".to_string(),
            ".pi".to_string(),
            ".trash".to_string(),
            "_margins".to_string(),
            "__pycache__".to_string(),
            "build".to_string(),
            "dist".to_string(),
            "node_modules".to_string(),
            "target".to_string(),
            "templates".to_string(),
        ]
    }

    #[test]
    fn structural_folder_excludes_descendants() {
        assert!(is_structural_folder(".claude"));
        assert!(is_structural_folder(".claude/commands"));
        assert!(is_structural_folder("projects/.obsidian/cache"));
        assert!(is_structural_folder("work/templates/client"));
        assert!(is_structural_folder(".agents/runs"));
        assert!(is_structural_folder("notes/.pi/agents"));
        assert!(is_structural_folder("projects/target/debug"));
        assert!(is_structural_folder("build/generated"));
        assert!(is_structural_folder("inbox/.aside"));
        assert!(is_structural_folder("_margins"));
        assert!(is_structural_folder("demo/synthetic-rosebud-vault"));
        assert!(is_structural_folder("obsidian-demo-vault"));
        assert!(!is_structural_folder("people"));
        assert!(!is_structural_folder("projects/client-a"));
    }

    #[test]
    fn suggested_folders_filters_structural_descendants() {
        let mut counts = HashMap::new();
        counts.insert("emails".to_string(), 5);
        counts.insert(".claude/commands".to_string(), 10);
        counts.insert(".agents/runs".to_string(), 9);
        counts.insert("people".to_string(), 3);
        counts.insert("target/debug".to_string(), 8);
        counts.insert("inbox/.aside".to_string(), 7);
        counts.insert("demo".to_string(), 6);

        let folders = suggested_folders(&counts);

        assert_eq!(folders, vec!["emails".to_string(), "people".to_string()]);
    }

    #[test]
    fn suggested_folders_fold_descendant_prefixes_into_base_folders() {
        let mut counts = HashMap::new();
        counts.insert("readwise".to_string(), 10);
        counts.insert("readwise/articles".to_string(), 10);
        counts.insert("projects".to_string(), 7);
        counts.insert("projects/done".to_string(), 7);
        counts.insert("interview_prep".to_string(), 5);
        counts.insert("interview_prep/system design".to_string(), 5);

        let folders = suggested_folders(&counts);

        assert_eq!(
            folders,
            vec![
                "readwise".to_string(),
                "projects".to_string(),
                "interview_prep".to_string(),
            ]
        );
    }

    #[test]
    fn flat_vault_suggests_root_folder_entity() {
        let docs = vec![doc(Vec::new()), doc(Vec::new())];

        let entities = suggested_entities(
            &docs,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &[],
        );

        assert_eq!(entities.first().map(String::as_str), Some("folder:."));
    }

    #[test]
    fn scan_json_includes_rich_evidence_and_legacy_fields() {
        let vault = TempDir::new().unwrap();
        let mut fm = Frontmatter {
            title: Some("Alice Project".to_string()),
            tags: vec!["project/x".to_string()],
            ..Frontmatter::default()
        };
        fm.aliases.push("AP".to_string());

        let docs = vec![
            doc_with(
                vec!["projects", "projects/x"],
                vec!["project", "project/x"],
                vec!["alice"],
                Some(fm),
                "projects/x/plan.md",
                1_700_000_000_000,
                "---\ntitle: Alice Project\n---\n#project/x working with [[alice]] on launch notes",
            ),
            doc_with(
                vec!["projects", "projects/x"],
                vec!["project"],
                vec!["alice"],
                None,
                "projects/x/update.md",
                1_700_000_100_000,
                "Follow-up with [[alice]] about #project details",
            ),
        ];

        let config_path = vault.path().join("margins-config.toml");
        let suggestion = build_scan_suggestion(
            vault.path(),
            &config_path,
            &WorkspacePolicy::default(),
            &docs,
        )
        .unwrap();
        let value = serde_json::to_value(&suggestion).unwrap();

        assert_eq!(value["files"], 2);
        assert!(value["entities"].is_array());
        assert_eq!(
            value["excluded_folders"],
            serde_json::json!([".git", "node_modules"])
        );
        assert_eq!(value["schema_version"], "scan.v2");
        assert_eq!(value["summary"]["file_count"], 2);
        assert!(value["top_entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["spec"] == "folder:projects"));
        assert!(value["top_tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["spec"] == "#project"));
        assert!(value["top_links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["spec"] == "[[alice]]"));
        assert!(!value["frontmatter_samples"].as_array().unwrap().is_empty());
        assert!(!value["entity_samples"].as_array().unwrap().is_empty());
        assert!(!value["sample_files"].as_array().unwrap().is_empty());
        assert!(!value["available_profiles"].as_array().unwrap().is_empty());
    }

    #[test]
    fn curation_candidates_expose_name_neutral_evidence_for_agent_review() {
        let vault = TempDir::new().unwrap();
        let docs = vec![
            doc_with(
                vec!["projects"],
                Vec::new(),
                Vec::new(),
                None,
                "projects/one.md",
                1_700_000_000_000,
                "Project one",
            ),
            doc_with(
                vec!["projects"],
                Vec::new(),
                Vec::new(),
                None,
                "projects/two.md",
                1_700_000_100_000,
                "Project two",
            ),
            doc_with(
                vec!["projects"],
                Vec::new(),
                Vec::new(),
                None,
                "projects/three.md",
                1_700_000_200_000,
                "Project three",
            ),
            doc_with(
                vec!["research"],
                Vec::new(),
                Vec::new(),
                None,
                "research/one.md",
                1_700_000_300_000,
                "Research one",
            ),
            doc_with(
                vec!["research"],
                Vec::new(),
                Vec::new(),
                None,
                "research/two.md",
                1_700_000_400_000,
                "Research two",
            ),
            doc_with(
                vec!["meetings"],
                Vec::new(),
                vec!["alice"],
                None,
                "meetings/check-in.md",
                1_700_000_500_000,
                "Check in with [[alice]]",
            ),
            doc_with(
                vec!["people"],
                Vec::new(),
                Vec::new(),
                None,
                "people/Alice.md",
                1_700_000_600_000,
                "Alice prefers concrete follow-up before the next meeting.",
            ),
            doc_with(
                vec!["client-history"],
                Vec::new(),
                Vec::new(),
                None,
                "client-history/Acme.md",
                1_700_000_700_000,
                "Priya at Acme wants the next conversation to address trust after the delayed launch.",
            ),
        ];
        let suggestion = build_scan_suggestion(
            vault.path(),
            &vault.path().join("config.toml"),
            &WorkspacePolicy::default(),
            &docs,
        )
        .unwrap();

        assert_eq!(
            suggestion
                .top_folders
                .iter()
                .take(2)
                .map(|entity| entity.name.as_str())
                .collect::<Vec<_>>(),
            vec!["projects", "research"]
        );
        let candidate = suggestion
            .entity_curation_candidates
            .iter()
            .find(|candidate| candidate.evidence.spec == "folder:people")
            .expect("all bounded folder evidence should be available for agent review");
        assert_eq!(candidate.page_entities, vec!["alice"]);
        assert!(!candidate.expands_automatically);
        assert_eq!(
            candidate.expansion.mode,
            FolderExpansionMode::ExplicitAvailable
        );
        assert!(candidate.expansion.explicit_expandable_available);
        assert_eq!(candidate.expansion.file_count, 1);
        assert_eq!(candidate.expansion.linked_child_count, 1);
        assert_eq!(candidate.expansion.active_linked_child_count, 0);
        assert_eq!(
            candidate.expansion.reason,
            "linked_child_pages_below_automatic_threshold"
        );
        assert!(candidate
            .expansion
            .representative_page_entities
            .iter()
            .any(|entity| entity.name == "alice" && entity.spec == "[[alice]]"));
        assert!(candidate
            .representative_samples
            .iter()
            .any(|sample| sample.excerpt.contains("concrete follow-up")));
        let unconventional_candidate = suggestion
            .entity_curation_candidates
            .iter()
            .find(|candidate| candidate.evidence.spec == "folder:client-history")
            .expect("curation evidence must not depend on a folder-name taxonomy");
        assert!(unconventional_candidate
            .representative_samples
            .iter()
            .any(|sample| sample.excerpt.contains("address trust")));
        assert!(suggestion
            .coverage_entities
            .iter()
            .any(|entity| entity.spec == "folder:meetings"));

        let serialized = serde_json::to_value(&suggestion).unwrap();
        for folder in ["people", "client-history"] {
            let entry = serialized["entity_curation_candidates"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["name"] == folder)
                .unwrap();
            assert!(entry.get("relationship_name_hint").is_none());
            assert!(entry.get("profile_hint").is_none());
            assert!(entry.get("profile_hint_basis").is_none());
            assert!(entry.get("expansion").is_some());
        }
    }

    #[test]
    fn people_like_curation_candidates_are_self_contained_for_expansion_judgment() {
        let vault = TempDir::new().unwrap();
        let docs = vec![
            doc_with(
                vec!["relationships"],
                Vec::new(),
                Vec::new(),
                None,
                "relationships/Ada Lovelace.md",
                1_700_000_000_000,
                "Ada likes concise prep notes before a call.",
            ),
            doc_with(
                vec!["relationships"],
                Vec::new(),
                Vec::new(),
                None,
                "relationships/Grace Hopper.md",
                1_700_000_010_000,
                "Grace wants follow-up framed around decisions.",
            ),
            doc_with(
                vec!["meetings"],
                Vec::new(),
                vec!["ada lovelace", "grace hopper"],
                None,
                "meetings/one.md",
                1_700_000_020_000,
                "Met with [[ada lovelace]] and [[grace hopper]].",
            ),
            doc_with(
                vec!["meetings"],
                Vec::new(),
                vec!["ada lovelace", "grace hopper"],
                None,
                "meetings/two.md",
                1_700_000_030_000,
                "Prepared next steps for [[ada lovelace]] and [[grace hopper]].",
            ),
            doc_with(
                vec!["projects"],
                Vec::new(),
                vec!["ada lovelace", "grace hopper"],
                None,
                "projects/launch.md",
                1_700_000_040_000,
                "Launch questions from [[ada lovelace]] and [[grace hopper]].",
            ),
        ];
        let suggestion = build_scan_suggestion(
            vault.path(),
            &vault.path().join("config.toml"),
            &WorkspacePolicy::default(),
            &docs,
        )
        .unwrap();
        let serialized = serde_json::to_value(&suggestion).unwrap();
        let relationships = serialized["entity_curation_candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["spec"] == "folder:relationships")
            .expect("relationships candidate");

        assert_eq!(relationships["expands_automatically"], true);
        assert_eq!(relationships["expansion"]["mode"], "automatic");
        assert_eq!(relationships["expansion"]["expands_automatically"], true);
        assert_eq!(
            relationships["expansion"]["explicit_expandable_available"],
            false
        );
        assert_eq!(relationships["expansion"]["file_count"], 2);
        assert_eq!(relationships["expansion"]["linked_child_count"], 2);
        assert_eq!(relationships["expansion"]["active_linked_child_count"], 2);
        assert_eq!(relationships["expansion"]["active_link_density"], 1.0);
        assert_eq!(
            relationships["expansion"]["automatic_density_threshold"],
            EXPANDABLE_DENSITY_THRESHOLD
        );
        assert_eq!(
            relationships["expansion"]["reason"],
            "active_child_density_meets_automatic_threshold"
        );
        let page_entities = relationships["expansion"]["representative_page_entities"]
            .as_array()
            .unwrap();
        assert!(page_entities.iter().any(|entry| {
            entry["name"] == "ada lovelace"
                && entry["spec"] == "[[ada lovelace]]"
                && entry["source_ref"] == "relationships/Ada Lovelace.md"
        }));
        assert!(page_entities.iter().any(|entry| {
            entry["name"] == "grace hopper"
                && entry["spec"] == "[[grace hopper]]"
                && entry["source_ref"] == "relationships/Grace Hopper.md"
        }));
        assert!(relationships["representative_samples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|sample| sample["excerpt"].as_str().unwrap().contains("concise prep")));
    }

    #[test]
    fn expansion_evidence_preserves_folder_size_without_linked_children() {
        let expansion = folder_expansion_evidence(None, 7);

        assert_eq!(expansion.mode, FolderExpansionMode::NotApplicable);
        assert_eq!(expansion.file_count, 7);
        assert_eq!(expansion.reason, "no_linked_child_pages");
    }

    #[test]
    fn proposed_exclusions_preserve_user_folders_and_add_only_observed_structural_noise() {
        let docs = vec![
            doc_with(
                vec!["templates"],
                Vec::new(),
                Vec::new(),
                None,
                "templates/meeting.md",
                1_700_000_000_000,
                "Meeting template",
            ),
            doc_with(
                vec!["projects"],
                Vec::new(),
                Vec::new(),
                None,
                "projects/atlas.md",
                1_700_000_000_000,
                "Project note",
            ),
        ];
        let policy = WorkspacePolicy {
            excluded_folders: vec!["old-archive".to_string()],
            ..WorkspacePolicy::default()
        };

        let excluded = proposed_excluded_folders(&policy, &docs);

        assert!(excluded.contains(&"old-archive".to_string()));
        assert!(excluded.contains(&"templates".to_string()));
        assert!(!excluded.contains(&"projects".to_string()));
    }

    #[test]
    fn proposed_exclusions_include_excalidraw_and_qwen_enzyme_benchmark_artifacts() {
        let docs = vec![
            doc_with(
                vec!["Excalidraw"],
                Vec::new(),
                Vec::new(),
                None,
                "Excalidraw/sketch.md",
                1_700_000_000_000,
                "Drawing data",
            ),
            doc_with(
                vec!["qwen-enzyme-7b-benchmark"],
                Vec::new(),
                Vec::new(),
                None,
                "qwen-enzyme-7b-benchmark/result.md",
                1_700_000_000_000,
                "Evaluation artifact",
            ),
            doc_with(
                vec!["projects"],
                Vec::new(),
                Vec::new(),
                None,
                "projects/atlas.md",
                1_700_000_000_000,
                "Project note",
            ),
        ];

        let excluded = proposed_excluded_folders(&WorkspacePolicy::default(), &docs);

        assert!(excluded.contains(&"Excalidraw".to_string()));
        assert!(excluded.contains(&"qwen-enzyme-7b-benchmark".to_string()));
        assert!(!excluded.contains(&"projects".to_string()));
    }

    #[test]
    fn proposed_exclusions_depend_on_scanned_vault_evidence() {
        let policy = WorkspacePolicy::default();
        let healthy_docs = vec![doc_with(
            vec!["projects"],
            Vec::new(),
            Vec::new(),
            None,
            "projects/atlas.md",
            1_700_000_000_000,
            "Project note",
        )];
        let noisy_docs = vec![doc_with(
            vec!["templates"],
            Vec::new(),
            Vec::new(),
            None,
            "templates/meeting.md",
            1_700_000_000_000,
            "Template note",
        )];

        let healthy = proposed_excluded_folders(&policy, &healthy_docs);
        let noisy = proposed_excluded_folders(&policy, &noisy_docs);

        assert_eq!(healthy, policy.excluded_folders);
        assert!(noisy.contains(&"templates".to_string()));
        assert_eq!(noisy.len(), policy.excluded_folders.len() + 1);
    }

    #[test]
    fn structural_descendant_proposes_nearest_observed_noise_folder() {
        let docs = vec![doc_with(
            vec!["work", "work/templates", "work/templates/client"],
            Vec::new(),
            Vec::new(),
            None,
            "work/templates/client/agenda.md",
            1_700_000_000_000,
            "Client agenda template",
        )];

        assert_eq!(
            observed_structural_exclusions(&docs),
            vec!["work/templates".to_string()]
        );
    }

    #[test]
    fn proposed_exclusions_do_not_duplicate_an_existing_observed_path() {
        let docs = vec![doc_with(
            vec!["Work", "Work/templates"],
            Vec::new(),
            Vec::new(),
            None,
            "Work/templates/agenda.md",
            1_700_000_000_000,
            "Reusable agenda",
        )];
        let policy = WorkspacePolicy {
            excluded_folders: vec!["Work/templates".to_string()],
            ..WorkspacePolicy::default()
        };

        assert_eq!(
            proposed_excluded_folders(&policy, &docs),
            policy.excluded_folders
        );
    }

    #[test]
    fn scan_json_includes_folder_page_entity_evidence() {
        let _lock = crate::test_process_env_lock().lock().unwrap();
        let old = std::env::var("MARGINS_HOME").ok();
        let home = TempDir::new().unwrap();
        // SAFETY: serialized by ENV_LOCK.
        unsafe { std::env::set_var("MARGINS_HOME", home.path()) };

        let vault = TempDir::new().unwrap();
        let docs = vec![
            doc_with(
                vec!["people"],
                Vec::new(),
                Vec::new(),
                None,
                "people/Alice.md",
                1_700_000_000_000,
                "Alice profile",
            ),
            doc_with(
                vec!["people"],
                Vec::new(),
                Vec::new(),
                None,
                "people/Bob.md",
                1_700_000_010_000,
                "Bob profile",
            ),
            doc_with(
                vec!["projects"],
                Vec::new(),
                vec!["alice", "bob"],
                None,
                "projects/launch.md",
                1_700_000_020_000,
                "Working with [[alice]] and [[bob]]",
            ),
            doc_with(
                vec!["meetings"],
                Vec::new(),
                vec!["alice", "bob"],
                None,
                "meetings/one.md",
                1_700_000_030_000,
                "Talked to [[alice]] and [[bob]]",
            ),
            doc_with(
                vec!["meetings"],
                Vec::new(),
                vec!["alice", "bob"],
                None,
                "meetings/two.md",
                1_700_000_040_000,
                "Follow-up with [[alice]] and [[bob]]",
            ),
        ];

        let config_path = home.path().join("config.toml");
        let suggestion = build_scan_suggestion(
            vault.path(),
            &config_path,
            &WorkspacePolicy::default(),
            &docs,
        )
        .unwrap();
        let value = serde_json::to_value(&suggestion).unwrap();
        let groups = value["folder_page_entities"].as_array().unwrap();
        let people = groups
            .iter()
            .find(|group| group["folder"] == "people")
            .expect("people page-entity group");

        assert_eq!(people["expandable_candidate"], true);
        assert!(people["page_entities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["name"] == "alice" && entry["spec"] == "[[alice]]" }));
        assert!(value["folder_stats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                entry["folder"] == "people"
                    && entry["expandable_candidate"] == true
                    && entry["page_entities"]
                        .as_array()
                        .unwrap()
                        .contains(&serde_json::json!("alice"))
            }));
        assert!(
            !suggestion
                .entities
                .iter()
                .any(|entity| entity == "[[alice]]"),
            "page link covered by selected folder should not be persisted: {:?}",
            suggestion.entities
        );

        restore_env(old);
    }

    #[test]
    fn scan_entity_curation_keeps_suggestions_unpersisted() {
        let home = TempDir::new().unwrap();
        let vault = TempDir::new().unwrap();

        let files = [
            ("readwise/articles/a.md", "Article about [[alice]]"),
            ("projects/done/b.md", "Project notes for [[alice]]"),
            (
                "interview_prep/system design/c.md",
                "System design with [[alice]]",
            ),
            ("people/Alice.md", "Alice profile"),
        ];
        for (relative, content) in files {
            let path = vault.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        let workspace = margins_workflows::workspace::create_workspace(
            home.path(),
            "practice",
            None,
            vault.path(),
        )
        .unwrap();
        let suggestion = execute_scan(&workspace).unwrap();
        let value = serde_json::to_value(&suggestion).unwrap();
        assert_eq!(value["status"], "suggested");
        let refs = &suggestion.entities;
        assert!(refs.contains(&"folder:readwise".to_string()), "{refs:?}");
        assert!(refs.contains(&"folder:projects".to_string()), "{refs:?}");
        assert!(
            refs.contains(&"folder:interview_prep".to_string()),
            "{refs:?}"
        );
        assert!(
            !refs.contains(&"folder:readwise/articles".to_string()),
            "{refs:?}"
        );
        assert!(
            !refs.contains(&"folder:projects/done".to_string()),
            "{refs:?}"
        );
        assert!(
            !refs.contains(&"folder:interview_prep/system design".to_string()),
            "{refs:?}"
        );
        assert!(!refs.contains(&"[[alice]]".to_string()), "{refs:?}");
        let raw = std::fs::read_to_string(&workspace.config_path).unwrap();
        let persisted: margins_workflows::workspace::WorkspaceConfig =
            toml::from_str(&raw).unwrap();
        assert!(persisted.policy.entities.is_empty());
        assert!(!raw.contains("folder:readwise"));
        assert!(!home.path().join("config.toml").exists());
    }

    #[test]
    fn scan_preview_is_read_only_and_ignores_external_enzyme_config() {
        let _lock = crate::test_process_env_lock().lock().unwrap();
        let old_enzyme = std::env::var("ENZYME_HOME").ok();
        let root = TempDir::new().unwrap();
        let margins_home = root.path().join("margins-home");
        let enzyme_home = root.path().join("enzyme-home");
        let vault = root.path().join("vault");
        std::fs::create_dir_all(vault.join("projects")).unwrap();
        std::fs::create_dir_all(&enzyme_home).unwrap();
        std::fs::write(
            vault.join("projects/plan.md"),
            "#customer Project plan for [[Rui Tan]]",
        )
        .unwrap();
        std::fs::write(enzyme_home.join("config.toml"), "[llm]\nmode = \"local\"\n").unwrap();
        let workspace =
            margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &vault)
                .unwrap();
        let before = std::fs::read_to_string(&workspace.config_path).unwrap();
        // SAFETY: serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("ENZYME_HOME", &enzyme_home);
        }

        run_scan(&workspace).unwrap();

        assert_eq!(
            std::fs::read_to_string(&workspace.config_path).unwrap(),
            before
        );
        assert!(!vault.join(".margins").exists());
        assert!(enzyme_home.join("config.toml").exists());
        // SAFETY: serialized by ENV_LOCK.
        unsafe {
            match old_enzyme {
                Some(value) => std::env::set_var("ENZYME_HOME", value),
                None => std::env::remove_var("ENZYME_HOME"),
            }
        }
    }
}
