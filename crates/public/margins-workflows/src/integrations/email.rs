//! Gmail email connector backed by a narrow thread transport.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

const AUTOMATED_SCORE_THRESHOLD: f64 = 0.55;
const SURVEY_THREAD_FETCH_CONCURRENCY: usize = 8;
const THREAD_FETCH_WAVE_DELAY: Duration = Duration::from_millis(3_200);
#[cfg(test)]
const MAX_QUOTA_RETRIES: usize = 5;

use super::connector::Connector;
use super::store::IntegrationsStore;
use super::types::{
    ConnectorCtx, CurationObservation, HealthReport, InferredEntity, KindCounts, ParticipantThread,
    PersonIdentity, RawItemDraft, ReconcileResult, RunManifest, SurveyRange, ThreadEvidence,
};
use crate::workspace::{GmailCollectionSelector, WorkspaceMutationError};

pub const EMAIL_CONNECTOR_ID: &str = "email";
const SEARCH_PAGE_SIZE: u64 = 100;
const SAMPLE_SENT: &str = "sent";
const SAMPLE_RECENT: &str = "recent";

/// A noise-filtered correspondent that may participate in the ordinary recall
/// entity surface. `identity_key` is always a normalized email or domain; the
/// display label is presentation-only and never participates in identity
/// merging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CorrespondentEntity {
    pub identity_key: String,
    pub label: String,
    pub kind: &'static str,
    pub thread_count: u32,
    pub weight: u64,
    pub last_interaction: Option<DateTime<Utc>>,
}

/// Automatic correspondent selection together with the raw link values that
/// must be kept out of engine-side frequency selection. Suppressed values stay
/// in indexed documents as evidence; only their entity candidacy is removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CorrespondentEntitySelection {
    pub entities: Vec<CorrespondentEntity>,
    pub excluded_links: Vec<String>,
    pub suppressed_count: usize,
    pub correspondents_considered: usize,
}

impl CorrespondentEntity {
    pub fn entity_ref(&self) -> String {
        format!("[[{}]]", self.identity_key)
    }
}

/// Encode one thread's bounded observable activity. Aggregation decodes these
/// components and orders their sums as a tuple; encoded scores are never added
/// together as a cross-participant ranking value.
fn correspondent_weight(user_replied: u32, user_sent: u32, thread_count: u32) -> u64 {
    u64::from(user_replied.min(999)) * 1_000_000
        + u64::from(user_sent.min(999)) * 1_000
        + u64::from(thread_count.min(999))
}

/// Derive automatic entities from retained email evidence. Approval is not
/// consulted: it is an operational activation receipt, while Workspace desired
/// state owns corpus membership and KnowledgePolicy owns entity curation.
pub fn automatic_correspondent_entities(
    workspace_state_dir: &Path,
    account: &str,
) -> Result<Vec<CorrespondentEntity>> {
    Ok(automatic_correspondent_entity_selection(workspace_state_dir, account)?.entities)
}

/// Derive both the ranked entities and the hard engine exclusions from the
/// same evidence snapshot, so automated/public-mail identities cannot be
/// resurrected by raw occurrence frequency.
pub fn automatic_correspondent_entity_selection(
    workspace_state_dir: &Path,
    account: &str,
) -> Result<CorrespondentEntitySelection> {
    let store = IntegrationsStore::open(workspace_state_dir)?;
    let ctx = ConnectorCtx {
        vault_root: workspace_state_dir.to_path_buf(),
        connector_id: EMAIL_CONNECTOR_ID.to_string(),
        account: account.trim().to_string(),
        command_path: None,
    };
    let ranked = store.ranked_participants(&ctx)?;
    let associations = store.participant_threads(&ctx)?;
    let mut excluded = BTreeSet::from([
        account.trim().to_ascii_lowercase(),
        normalize_email(account),
    ]);
    if let Some(observation) =
        store.latest_curation_observation_for_account(EMAIL_CONNECTOR_ID, &ctx.account)?
    {
        extend_observation_exclusions(&mut excluded, &observation);
    }

    let mut entities = Vec::new();
    for row in &ranked {
        if excluded.contains(&row.participant) {
            continue;
        }
        let thread_count = associations
            .iter()
            .filter(|association| association.participant == row.participant)
            .count() as u32;
        entities.push(CorrespondentEntity {
            identity_key: row.participant.clone(),
            label: row.participant.clone(),
            kind: "person",
            thread_count,
            weight: row.aggregate_sampling_score,
            last_interaction: Some(row.last_interaction),
        });
    }

    // `ranked` already uses the uncapped reply/sent/thread tuple. Preserve that
    // exact order when capped generic importance values tie.
    let correspondents_considered = ranked.len();
    let suppressed_count = excluded.len();
    excluded.retain(|value| !value.is_empty());
    Ok(CorrespondentEntitySelection {
        entities,
        excluded_links: excluded.into_iter().collect(),
        suppressed_count,
        correspondents_considered,
    })
}

fn extend_observation_exclusions(excluded: &mut BTreeSet<String>, report: &CurationObservation) {
    for (proposal, flags) in &report.proposal_flags {
        if !flags.iter().any(|flag| flag == "likely-automated") {
            continue;
        }
        if let Some(identity) = proposal
            .strip_prefix("person:")
            .map(normalize_email)
            .or_else(|| proposal.strip_prefix("domain:").map(normalize_domain))
        {
            excluded.insert(identity);
        }
    }
    for proposal in &report.proposed_include {
        if let Some(email) = proposal.strip_prefix("person:").map(normalize_email) {
            if let Some(domain) =
                email_domain(&email).filter(|domain| is_public_mail_domain(domain))
            {
                excluded.insert(domain.to_string());
            }
        } else if let Some(domain) = proposal.strip_prefix("domain:").map(normalize_domain) {
            if is_public_mail_domain(&domain) {
                excluded.insert(domain);
            }
        }
    }
}

/// Read-only Gmail connector transport. OAuth, refresh, pagination, quotas and
/// HTTP belong to the injected driver; this module owns materialization.
pub trait GmailThreadTransport: Send + Sync {
    fn search_page(
        &self,
        ctx: &ConnectorCtx,
        query: &str,
        max_results: u64,
        page_token: Option<&str>,
    ) -> Result<GmailSearchPage>;

    fn fetch_thread(&self, ctx: &ConnectorCtx, thread_id: &str) -> Result<FetchedThread>;
}

#[derive(Debug, Clone)]
pub struct GoogleEmailConnector<T> {
    selector: GmailCollectionSelector,
    transport: T,
}

impl<T: GmailThreadTransport> GoogleEmailConnector<T> {
    pub fn new(selector: GmailCollectionSelector, transport: T) -> Self {
        Self {
            selector,
            transport,
        }
    }

    fn report_for_snapshot(
        &self,
        ctx: &ConnectorCtx,
        samples: &SurveySamples,
        surveyed_at: DateTime<Utc>,
    ) -> CurationObservation {
        let survey = survey_threads_with_samples(ctx, &samples.threads, &samples.thread_samples);
        let observed_range =
            survey
                .occurred_from
                .zip(survey.occurred_to)
                .map(|(occurred_from, occurred_to)| SurveyRange {
                    occurred_from,
                    occurred_to,
                });
        CurationObservation {
            connector_id: ctx.connector_id.clone(),
            account: ctx.account.clone(),
            item_counts: KindCounts {
                counts: BTreeMap::from([("email".to_string(), samples.threads.len() as u64)]),
            },
            occurred_from: survey.occurred_from,
            occurred_to: survey.occurred_to,
            observation_window: requested_survey_range(self.selector.backfill_days, surveyed_at),
            observed_range,
            sample_observed_ranges: sample_observed_ranges(
                &samples.threads,
                &samples.thread_samples,
            ),
            observation_call_count: Some(samples.call_count),
            detected_accounts: vec![ctx.account.clone()],
            inferred_people: survey.people,
            inferred_orgs: survey.orgs,
            proposed_include: survey.proposed_include,
            proposed_exclude: survey.proposed_exclude,
            proposal_flags: survey.proposal_flags,
            proposal_evidence: survey.proposal_evidence,
            decisions_required: survey.decisions_required,
        }
    }

    fn search_query(&self) -> String {
        let temporal = format!("newer_than:{}d", self.selector.backfill_days);
        let query = self.selector.query.trim();
        if query.is_empty() {
            temporal
        } else {
            format!("{temporal} {query}")
        }
    }

    fn fetch_threads(
        &self,
        ctx: &ConnectorCtx,
        thread_ids: &[String],
    ) -> Result<Vec<FetchedThread>> {
        let mut threads = Vec::with_capacity(thread_ids.len());
        let chunks = thread_ids.chunks(SURVEY_THREAD_FETCH_CONCURRENCY);
        let chunk_count = chunks.len();
        for (chunk_index, chunk) in chunks.enumerate() {
            let fetched = std::thread::scope(|scope| {
                chunk
                    .iter()
                    .map(|thread_id| scope.spawn(|| self.transport.fetch_thread(ctx, thread_id)))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .map_err(|_| anyhow::anyhow!("survey thread fetch worker panicked"))?
                    })
                    .collect::<Result<Vec<_>>>()
            })?;
            threads.extend(fetched);
            if chunk_index + 1 < chunk_count {
                // Eight gets every 3.2 seconds is exactly 150/minute, Gmail's
                // 6,000-unit/user/minute ceiling at 40 units per threads.get.
                std::thread::sleep(THREAD_FETCH_WAVE_DELAY);
            }
        }
        Ok(threads)
    }

    fn search_survey_thread_ids(
        &self,
        ctx: &ConnectorCtx,
        query: &str,
    ) -> Result<(Vec<String>, u64)> {
        let mut thread_ids = Vec::new();
        let mut page_token = None;
        let mut seen_tokens = BTreeSet::new();
        let mut call_count = 0;
        loop {
            let page =
                self.transport
                    .search_page(ctx, query, SEARCH_PAGE_SIZE, page_token.as_deref())?;
            call_count += 1;
            thread_ids.extend(page.thread_ids);
            let Some(next) = page.next_page_token.filter(|value| !value.is_empty()) else {
                break;
            };
            if !seen_tokens.insert(next.clone()) {
                bail!("Gmail search repeated page token {next}");
            }
            page_token = Some(next);
        }
        Ok((thread_ids, call_count))
    }

    /// Fetch the complete bounded source snapshot used for evidence
    /// reconciliation. Incremental Gmail history is not authoritative for
    /// absence, so it must never drive replacement or deletion.
    fn fetch_complete_snapshot(&self, ctx: &ConnectorCtx) -> Result<SurveySamples> {
        let query = self.search_query();
        // Gmail's `from:me` operator follows owner aliases and selects any
        // thread containing owner-authored mail; it does not depend on the
        // mutable SENT label remaining on the message.
        let sent_query = format!("from:me {query}");
        let (sent_ids, sent_calls) = self.search_survey_thread_ids(ctx, &sent_query)?;
        let (recent_ids, recent_calls) = self.search_survey_thread_ids(ctx, &query)?;
        let mut thread_samples: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (sample, thread_ids) in [(SAMPLE_SENT, sent_ids), (SAMPLE_RECENT, recent_ids)] {
            for thread_id in thread_ids {
                thread_samples
                    .entry(thread_id)
                    .or_default()
                    .insert(sample.to_string());
            }
        }
        let thread_ids = thread_samples.keys().cloned().collect::<Vec<_>>();
        let threads = self.fetch_threads(ctx, &thread_ids)?;
        let next_history_id = threads
            .iter()
            .filter_map(|thread| thread.history_id.as_deref())
            .max_by(|left, right| compare_decimal_ids(left, right))
            .map(str::to_string);
        Ok(SurveySamples {
            call_count: sent_calls + recent_calls + threads.len() as u64,
            threads,
            thread_samples,
            next_history_id,
        })
    }
}

impl<T: GmailThreadTransport> Connector for GoogleEmailConnector<T> {
    fn reconcile(
        &self,
        ctx: &ConnectorCtx,
        expected_workspace_revision: Option<&str>,
    ) -> Result<ReconcileResult> {
        validate_email_ctx(ctx)?;
        let store = IntegrationsStore::open(&ctx.vault_root)?;
        let result: Result<ReconcileResult> = (|| {
            let surveyed_at = Utc::now();
            let samples = self.fetch_complete_snapshot(ctx)?;
            let mut stored: BTreeMap<String, StoredSurveyThread> = BTreeMap::new();
            for thread in &samples.threads {
                stored.insert(
                    thread.id.clone(),
                    StoredSurveyThread::from_fetched(
                        thread,
                        samples
                            .thread_samples
                            .get(&thread.id)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                );
            }
            let next_cursor = samples
                .next_history_id
                .as_ref()
                .map(|history_id| serde_json::json!({"history_id": history_id}));
            store.ingest_raw_connector_items(
                ctx,
                stored.values().map(StoredSurveyThread::raw_item).collect(),
            )?;
            let report = self.report_for_snapshot(ctx, &samples, surveyed_at);
            let (thread_rows, associations) = email_thread_snapshot(ctx, &samples.threads)?;
            let counts = store.replace_email_thread_snapshot_with_policy_report(
                ctx,
                thread_rows,
                associations,
                &report,
                ctx.command_path.as_deref(),
                &self.selector.materialization_fingerprint()?,
                surveyed_at,
                next_cursor.as_ref(),
                expected_workspace_revision,
            )?;
            let manifest = RunManifest {
                records_written: counts.threads_written,
                records_updated: counts.threads_updated,
                records_unchanged: counts.threads_unchanged,
                tombstones: counts.threads_deleted,
                ..RunManifest::default()
            };
            Ok(ReconcileResult {
                records_written: manifest.records_written,
                records_updated: manifest.records_updated,
                records_unchanged: manifest.records_unchanged,
                tombstones: manifest.tombstones,
                next_cursor,
                manifest,
            })
        })();
        if let Err(error) = &result {
            if error.downcast_ref::<WorkspaceMutationError>().is_none() {
                let _ = store.record_failed_reconcile(ctx, &format!("{error:#}"));
            }
        }
        result
    }

    fn health(&self, ctx: &ConnectorCtx) -> Result<HealthReport> {
        validate_email_ctx(ctx)?;
        IntegrationsStore::open(&ctx.vault_root)?.health_report(ctx)
    }
}

fn requested_survey_range(backfill_days: u32, occurred_to: DateTime<Utc>) -> Option<SurveyRange> {
    let occurred_from =
        occurred_to.checked_sub_signed(TimeDelta::days(i64::from(backfill_days)))?;
    Some(SurveyRange {
        occurred_from,
        occurred_to,
    })
}

#[derive(Debug, Deserialize)]
struct GmailThreadEnvelope {
    thread: GmailThreadRaw,
}

#[derive(Debug, Deserialize)]
struct GmailThreadRaw {
    id: String,
    #[serde(default, rename = "historyId")]
    history_id: Option<String>,
    #[serde(default)]
    messages: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GmailMessage {
    pub(crate) id: String,
    #[serde(default, rename = "threadId")]
    pub(crate) thread_id: String,
    #[serde(default, rename = "internalDate")]
    pub(crate) internal_date: serde_json::Value,
    #[serde(default)]
    pub(crate) headers: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) body: String,
}

#[derive(Debug)]
pub(crate) struct FetchedMessage {
    pub(crate) parsed: GmailMessage,
    pub(crate) raw: serde_json::Value,
}

#[derive(Debug)]
pub struct FetchedThread {
    pub(crate) id: String,
    pub(crate) messages: Vec<FetchedMessage>,
    pub(crate) history_id: Option<String>,
}

#[derive(Debug)]
pub struct GmailSearchPage {
    pub thread_ids: Vec<String>,
    pub next_page_token: Option<String>,
}

#[derive(Debug)]
struct SurveySamples {
    threads: Vec<FetchedThread>,
    thread_samples: BTreeMap<String, BTreeSet<String>>,
    call_count: u64,
    next_history_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSurveyThread {
    thread: serde_json::Value,
    #[serde(default)]
    samples: BTreeSet<String>,
}

impl StoredSurveyThread {
    fn from_fetched(thread: &FetchedThread, samples: BTreeSet<String>) -> Self {
        Self {
            thread: serde_json::json!({
                "thread": {
                    "id": thread.id,
                    "historyId": thread.history_id,
                    "messages": thread.messages.iter().map(|message| &message.raw).collect::<Vec<_>>()
                }
            }),
            samples,
        }
    }

    fn raw_item(&self) -> RawItemDraft {
        let source_id = self
            .thread
            .pointer("/thread/id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        RawItemDraft {
            source_id,
            payload: serde_json::to_value(self).expect("stored survey thread serializes"),
        }
    }
}

#[derive(Default)]
struct SurveyEvidence {
    occurred_from: Option<DateTime<Utc>>,
    occurred_to: Option<DateTime<Utc>>,
    people: Vec<InferredEntity>,
    orgs: Vec<InferredEntity>,
    proposed_include: Vec<String>,
    proposed_exclude: Vec<String>,
    proposal_flags: BTreeMap<String, Vec<String>>,
    proposal_evidence: BTreeMap<String, serde_json::Value>,
    decisions_required: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EmailProposalEvidence {
    threads: u32,
    thread_count: u32,
    user_replied: u32,
    user_sent: u32,
    last_interaction: Option<DateTime<Utc>>,
    automated_signals: BTreeSet<String>,
    samples: BTreeSet<String>,
}

impl EmailProposalEvidence {
    fn observe(
        &mut self,
        user_replied: u32,
        user_sent: u32,
        last_interaction: Option<DateTime<Utc>>,
        automated_signals: impl IntoIterator<Item = String>,
        samples: impl IntoIterator<Item = String>,
    ) {
        self.threads += 1;
        self.thread_count += 1;
        self.user_replied += user_replied;
        self.user_sent += user_sent;
        if let Some(last_interaction) = last_interaction {
            self.last_interaction = Some(
                self.last_interaction
                    .map_or(last_interaction, |current| current.max(last_interaction)),
            );
        }
        self.automated_signals.extend(automated_signals);
        self.samples.extend(samples);
    }
}

pub fn parse_gmail_thread_from_value(
    value: serde_json::Value,
    expected_id: &str,
) -> Result<FetchedThread> {
    let envelope: GmailThreadEnvelope = serde_json::from_value(value)
        .context("Gmail thread lookup returned an unexpected response")?;
    if envelope.thread.id != expected_id {
        bail!(
            "Gmail returned thread {} while {} was requested",
            envelope.thread.id,
            expected_id
        );
    }
    let mut messages = Vec::with_capacity(envelope.thread.messages.len());
    for raw in envelope.thread.messages {
        let parsed: GmailMessage = serde_json::from_value(raw.clone())
            .context("Gmail thread contained an invalid message")?;
        if parsed.id.trim().is_empty() || parsed.thread_id != expected_id {
            bail!("Gmail thread contained a message with inconsistent identity");
        }
        messages.push(FetchedMessage { parsed, raw });
    }
    Ok(FetchedThread {
        id: envelope.thread.id,
        messages,
        history_id: envelope.thread.history_id,
    })
}

#[cfg(test)]
fn survey_threads(ctx: &ConnectorCtx, threads: &[FetchedThread]) -> SurveyEvidence {
    let samples = threads
        .iter()
        .map(|thread| {
            (
                thread.id.clone(),
                BTreeSet::from([SAMPLE_RECENT.to_string()]),
            )
        })
        .collect();
    survey_threads_with_samples(ctx, threads, &samples)
}

fn survey_threads_with_samples(
    ctx: &ConnectorCtx,
    threads: &[FetchedThread],
    thread_samples: &BTreeMap<String, BTreeSet<String>>,
) -> SurveyEvidence {
    let account = normalize_email(&ctx.account);
    let mut evidence = SurveyEvidence::default();
    let mut identities: BTreeMap<(String, Option<String>), BTreeSet<String>> = BTreeMap::new();
    let mut name_emails: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut domains: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut email_signals: BTreeMap<String, CorrespondenceSignals> = BTreeMap::new();
    let mut domain_signals: BTreeMap<String, CorrespondenceSignals> = BTreeMap::new();
    let mut email_proposal_evidence: BTreeMap<String, EmailProposalEvidence> = BTreeMap::new();
    let mut domain_proposal_evidence: BTreeMap<String, EmailProposalEvidence> = BTreeMap::new();

    for thread in threads {
        let samples = thread_samples.get(&thread.id).cloned().unwrap_or_default();
        let thread_automated_signals = thread_automated_signal_names(thread);
        let thread_automated = !thread_automated_signals.is_empty();
        let thread_list_unsubscribe = thread_has_list_unsubscribe(thread);
        let thread_identities = thread_external_emails(thread, &account);
        let last_interaction = thread
            .messages
            .iter()
            .filter_map(|message| message_datetime(&message.parsed))
            .max();
        for email in &thread_identities {
            let user_sent = thread_user_sent_count(thread, &account, email);
            let user_replied = thread_user_reply_count(thread, &account, email);
            email_signals.entry(email.clone()).or_default().observe(
                thread_automated,
                thread_list_unsubscribe,
                user_sent > 0,
                user_replied > 0,
                automated_address_heuristic(email),
            );
            let mut raw_signals = thread_automated_signals.clone();
            if automated_address_heuristic(email) {
                raw_signals.insert("automated-address-pattern".to_string());
            }
            email_proposal_evidence
                .entry(email.clone())
                .or_default()
                .observe(
                    user_replied,
                    user_sent,
                    last_interaction,
                    raw_signals,
                    samples.iter().cloned(),
                );
            if let Some(domain) = email_domain(email) {
                domain_signals
                    .entry(domain.to_string())
                    .or_default()
                    .observe(
                        thread_automated,
                        thread_list_unsubscribe,
                        user_sent > 0,
                        user_replied > 0,
                        automated_domain_heuristic(domain),
                    );
            }
        }
        let thread_domains = thread_identities
            .iter()
            .filter_map(|email| email_domain(email).map(str::to_string))
            .collect::<BTreeSet<_>>();
        for domain in thread_domains {
            let domain_emails = thread_identities
                .iter()
                .filter(|email| email_domain(email) == Some(domain.as_str()))
                .collect::<Vec<_>>();
            let user_sent = domain_emails
                .iter()
                .map(|email| thread_user_sent_count(thread, &account, email))
                .sum();
            let user_replied = domain_emails
                .iter()
                .map(|email| thread_user_reply_count(thread, &account, email))
                .sum();
            let mut raw_signals = thread_automated_signals.clone();
            if automated_domain_heuristic(&domain) {
                raw_signals.insert("automated-domain-pattern".to_string());
            }
            domain_proposal_evidence.entry(domain).or_default().observe(
                user_replied,
                user_sent,
                last_interaction,
                raw_signals,
                samples.iter().cloned(),
            );
        }
        for message in &thread.messages {
            if let Some(occurred) = message_datetime(&message.parsed) {
                evidence.occurred_from = Some(
                    evidence
                        .occurred_from
                        .map_or(occurred, |current| current.min(occurred)),
                );
                evidence.occurred_to = Some(
                    evidence
                        .occurred_to
                        .map_or(occurred, |current| current.max(occurred)),
                );
            }
            for person in message_participants(&message.parsed) {
                let normalized = person.email.as_deref().map(normalize_email);
                if normalized.as_deref() == Some(account.as_str()) {
                    continue;
                }
                let name_key = person.display_name.to_lowercase();
                let evidence_string = person
                    .email
                    .as_ref()
                    .map(|email| format!("{} <{}>", person.display_name, email))
                    .unwrap_or_else(|| person.display_name.clone());
                identities
                    .entry((name_key.clone(), normalized.clone()))
                    .or_default()
                    .insert(evidence_string);
                if let Some(email) = normalized {
                    name_emails
                        .entry(name_key)
                        .or_default()
                        .insert(email.clone());
                    if let Some(domain) = email_domain(&email) {
                        domains.entry(domain.to_string()).or_default().insert(email);
                    }
                }
            }
        }
    }

    let ambiguous_names: BTreeSet<_> = name_emails
        .iter()
        .filter(|(_, emails)| emails.len() > 1)
        .map(|(name, _)| name.clone())
        .collect();
    for ((name_key, email), values) in identities {
        let label = values.iter().next().cloned().unwrap_or(name_key.clone());
        let ambiguous = email.is_none() || ambiguous_names.contains(&name_key);
        evidence.people.push(InferredEntity {
            label: label.clone(),
            evidence: values.into_iter().collect(),
            ambiguous,
        });
        if ambiguous {
            evidence
                .proposed_exclude
                .push(format!("ambiguous-person:{label}"));
        } else if let Some(email) = email {
            let proposal = format!("person:{email}");
            if email_signals
                .get(&email)
                .is_some_and(|signals| signals.likely_automated())
            {
                evidence
                    .proposal_flags
                    .insert(proposal.clone(), vec!["likely-automated".to_string()]);
            }
            evidence.proposed_include.push(proposal);
        }
    }
    for name in ambiguous_names {
        evidence.decisions_required.push(format!(
            "Multiple email identities share the display name {name}; they will not be merged or proposed as a person match"
        ));
    }
    for (domain, emails) in domains {
        if is_public_mail_domain(&domain) {
            continue;
        }
        evidence.orgs.push(InferredEntity {
            label: domain.clone(),
            evidence: emails.into_iter().collect(),
            ambiguous: false,
        });
        let proposal = format!("domain:{domain}");
        if domain_signals
            .get(&domain)
            .is_some_and(|signals| signals.likely_automated())
        {
            evidence
                .proposal_flags
                .insert(proposal.clone(), vec!["likely-automated".to_string()]);
        }
        evidence.proposed_include.push(proposal);
    }
    evidence.people.sort_by(|a, b| a.label.cmp(&b.label));
    evidence.orgs.sort_by(|a, b| a.label.cmp(&b.label));
    evidence.proposed_include.sort_by(|left, right| {
        proposal_rank(right, &email_proposal_evidence, &domain_proposal_evidence)
            .cmp(&proposal_rank(
                left,
                &email_proposal_evidence,
                &domain_proposal_evidence,
            ))
            .then_with(|| {
                proposal_human_score(right, &email_signals, &domain_signals)
                    .cmp(&proposal_human_score(left, &email_signals, &domain_signals))
            })
            .then_with(|| {
                proposal_thread_recency_rank(
                    right,
                    &email_proposal_evidence,
                    &domain_proposal_evidence,
                )
                .cmp(&proposal_thread_recency_rank(
                    left,
                    &email_proposal_evidence,
                    &domain_proposal_evidence,
                ))
            })
            .then_with(|| left.cmp(right))
    });
    evidence.proposed_include.dedup();
    for proposal in &evidence.proposed_include {
        let raw = proposal
            .strip_prefix("person:")
            .and_then(|email| email_proposal_evidence.get(email))
            .or_else(|| {
                proposal
                    .strip_prefix("domain:")
                    .and_then(|domain| domain_proposal_evidence.get(domain))
            });
        if let Some(raw) = raw {
            evidence.proposal_evidence.insert(
                proposal.clone(),
                serde_json::to_value(raw).expect("email proposal evidence is serializable"),
            );
        }
    }
    evidence.proposed_exclude.sort();
    evidence.proposed_exclude.dedup();
    evidence
}

fn sample_observed_ranges(
    threads: &[FetchedThread],
    thread_samples: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, SurveyRange> {
    let mut bounds: BTreeMap<String, (DateTime<Utc>, DateTime<Utc>)> = BTreeMap::new();
    for thread in threads {
        let Some(samples) = thread_samples.get(&thread.id) else {
            continue;
        };
        for occurred in thread
            .messages
            .iter()
            .filter_map(|message| message_datetime(&message.parsed))
        {
            for sample in samples {
                bounds
                    .entry(sample.clone())
                    .and_modify(|(from, to)| {
                        *from = (*from).min(occurred);
                        *to = (*to).max(occurred);
                    })
                    .or_insert((occurred, occurred));
            }
        }
    }
    bounds
        .into_iter()
        .map(|(sample, (occurred_from, occurred_to))| {
            (
                sample,
                SurveyRange {
                    occurred_from,
                    occurred_to,
                },
            )
        })
        .collect()
}

fn proposal_rank(
    proposal: &str,
    email_evidence: &BTreeMap<String, EmailProposalEvidence>,
    domain_evidence: &BTreeMap<String, EmailProposalEvidence>,
) -> (u32, u32) {
    let evidence = proposal
        .strip_prefix("person:")
        .and_then(|email| email_evidence.get(email))
        .or_else(|| {
            proposal
                .strip_prefix("domain:")
                .and_then(|domain| domain_evidence.get(domain))
        });
    evidence.map_or((0, 0), |evidence| {
        (evidence.user_replied, evidence.user_sent)
    })
}

fn proposal_thread_recency_rank(
    proposal: &str,
    email_evidence: &BTreeMap<String, EmailProposalEvidence>,
    domain_evidence: &BTreeMap<String, EmailProposalEvidence>,
) -> (u32, Option<DateTime<Utc>>) {
    let evidence = proposal
        .strip_prefix("person:")
        .and_then(|email| email_evidence.get(email))
        .or_else(|| {
            proposal
                .strip_prefix("domain:")
                .and_then(|domain| domain_evidence.get(domain))
        });
    evidence.map_or((0, None), |evidence| {
        (evidence.thread_count, evidence.last_interaction)
    })
}

/// Canonical identity used by the email connector (including Gmail dot/plus
/// alias folding). Workspace composition uses the same identity for owner
/// exclusions across every connected account.
pub fn normalize_email_identity(value: &str) -> String {
    normalize_email(value)
}

#[derive(Debug, Clone, Copy, Default)]
struct CorrespondenceSignals {
    observations: u32,
    automated_observations: u32,
    list_unsubscribe_observations: u32,
    user_sent: bool,
    user_replied: bool,
    automated_address: bool,
}

impl CorrespondenceSignals {
    fn observe(
        &mut self,
        automated: bool,
        list_unsubscribe: bool,
        user_sent: bool,
        user_replied: bool,
        automated_address: bool,
    ) {
        self.observations += 1;
        self.automated_observations += u32::from(automated);
        self.list_unsubscribe_observations += u32::from(list_unsubscribe);
        self.user_sent |= user_sent;
        self.user_replied |= user_replied;
        self.automated_address |= automated_address;
    }

    fn automated_score(self) -> f64 {
        if self.observations == 0 {
            return 0.0;
        }
        let observations = f64::from(self.observations);
        let automated_fraction = f64::from(self.automated_observations) / observations;
        let unsubscribe_fraction = f64::from(self.list_unsubscribe_observations) / observations;
        let no_reply = if self.user_replied { 0.0 } else { 1.0 };
        let address = if self.automated_address { 1.0 } else { 0.0 };
        // Repeated observations dominate; address/header signals strengthen
        // the classification, while a real user reply is both a zeroed signal
        // here and a hard veto in `likely_automated` below.
        (automated_fraction * 0.55)
            + (unsubscribe_fraction * 0.15)
            + (address * 0.50)
            + (no_reply * 0.10)
    }

    fn likely_automated(self) -> bool {
        !self.user_replied && self.automated_score() >= AUTOMATED_SCORE_THRESHOLD
    }

    fn human_score(self) -> u32 {
        if self.user_replied {
            return 4_000;
        }
        if self.user_sent {
            return 3_000;
        }
        if self.automated_address {
            return if self.automated_observations > 0
                && self.automated_observations < self.observations
            {
                1_000
                    + ((1.0
                        - (f64::from(self.automated_observations) / f64::from(self.observations)))
                        * 999.0)
                        .round() as u32
            } else {
                0
            };
        }
        let human_fraction = if self.observations == 0 {
            0.0
        } else {
            1.0 - (f64::from(self.automated_observations) / f64::from(self.observations))
        };
        let tier = if self.automated_observations == self.observations {
            0
        } else {
            2_000
        };
        tier + (human_fraction * 999.0).round() as u32
    }
}

fn proposal_human_score(
    proposal: &str,
    email_signals: &BTreeMap<String, CorrespondenceSignals>,
    domain_signals: &BTreeMap<String, CorrespondenceSignals>,
) -> u32 {
    proposal
        .strip_prefix("person:")
        .and_then(|email| email_signals.get(email))
        .or_else(|| {
            proposal
                .strip_prefix("domain:")
                .and_then(|domain| domain_signals.get(domain))
        })
        .copied()
        .map_or(0, CorrespondenceSignals::human_score)
}

fn thread_automated_signal_names(thread: &FetchedThread) -> BTreeSet<String> {
    let mut signals = BTreeSet::new();
    for message in &thread.messages {
        let message = &message.parsed;
        if header(message, "list-unsubscribe").is_some() {
            signals.insert("list-unsubscribe".to_string());
        }
        if header(message, "auto-submitted").is_some_and(|value| !value.eq_ignore_ascii_case("no"))
        {
            signals.insert("auto-submitted".to_string());
        }
        if header(message, "precedence").is_some_and(|value| {
            ["bulk", "list", "junk"]
                .iter()
                .any(|marker| value.eq_ignore_ascii_case(marker))
        }) {
            signals.insert("bulk-precedence".to_string());
        }
        if message_participants_for_header(message, "from")
            .iter()
            .filter_map(|person| person.email.as_deref())
            .any(|email| {
                let local = email
                    .split_once('@')
                    .map_or(email, |(local, _)| local)
                    .to_ascii_lowercase();
                ["noreply", "no-reply", "do-not-reply", "donotreply"]
                    .iter()
                    .any(|marker| local.contains(marker))
            })
        {
            signals.insert("automated-sender-address".to_string());
        }
        if header(message, "subject").is_some_and(|subject| {
            let subject = subject.to_ascii_lowercase();
            [
                "[ci]",
                "ci failed",
                "ci passed",
                "build failed",
                "build succeeded",
                "checks failed",
                "checks passed",
                "continuous integration",
                "workflow run",
                "github notification",
                "unsubscribe",
                "daily digest",
                "statement is ready",
                "payment receipt",
                "payment confirmation",
                "order confirmation",
                "your tickets",
                "account alert",
                "verification code",
            ]
            .iter()
            .any(|marker| subject.contains(marker))
        }) {
            signals.insert("automated-subject".to_string());
        }
    }
    signals
}

fn thread_has_list_unsubscribe(thread: &FetchedThread) -> bool {
    thread
        .messages
        .iter()
        .any(|message| header(&message.parsed, "list-unsubscribe").is_some())
}

fn thread_external_emails(thread: &FetchedThread, account: &str) -> BTreeSet<String> {
    thread
        .messages
        .iter()
        .flat_map(|message| message_participants(&message.parsed))
        .filter_map(|person| person.email.map(|email| normalize_email(&email)))
        .filter(|email| email != account)
        .collect()
}

fn message_from_account(message: &GmailMessage, account: &str) -> bool {
    message_participants_for_header(message, "from")
        .iter()
        .filter_map(|person| person.email.as_deref())
        .any(|email| normalize_email(email) == account)
}

fn message_sent_to(message: &GmailMessage, target: &str) -> bool {
    ["to", "cc", "bcc"].into_iter().any(|header_name| {
        message_participants_for_header(message, header_name)
            .iter()
            .filter_map(|person| person.email.as_deref())
            .any(|email| normalize_email(email) == target)
    })
}

fn thread_user_sent_count(thread: &FetchedThread, account: &str, target: &str) -> u32 {
    thread
        .messages
        .iter()
        .filter(|message| {
            message_from_account(&message.parsed, account)
                && message_sent_to(&message.parsed, target)
        })
        .count() as u32
}

fn thread_user_reply_count(thread: &FetchedThread, account: &str, target: &str) -> u32 {
    let mut saw_target_sender = false;
    let mut replies = 0;
    for message in &thread.messages {
        let parsed = &message.parsed;
        if message_from_account(parsed, account) && message_sent_to(parsed, target) {
            if saw_target_sender
                || header(parsed, "in-reply-to").is_some()
                || header(parsed, "references").is_some()
                || header(parsed, "subject").is_some_and(|subject| {
                    subject.trim_start().to_ascii_lowercase().starts_with("re:")
                })
            {
                replies += 1;
            }
        } else if message_participants_for_header(parsed, "from")
            .iter()
            .filter_map(|person| person.email.as_deref())
            .any(|email| normalize_email(email) == target)
        {
            saw_target_sender = true;
        }
    }
    replies
}

fn automated_address_heuristic(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    let local = local.to_ascii_lowercase();
    ["noreply", "no-reply", "do-not-reply", "donotreply"]
        .iter()
        .any(|marker| local.contains(marker))
        || [
            "notification",
            "notifications",
            "mail",
            "mailer",
            "marketing",
            "newsletter",
            "promo",
            "updates",
        ]
        .iter()
        .any(|marker| {
            local == *marker
                || ['+', '-', '_']
                    .iter()
                    .any(|separator| local.starts_with(&format!("{marker}{separator}")))
        })
        || automated_domain_heuristic(domain)
}

fn automated_domain_heuristic(domain: &str) -> bool {
    let domain = normalize_domain(domain);
    let first_label = domain.split('.').next().unwrap_or("");
    [
        "mail",
        "email",
        "mailer",
        "marketing",
        "newsletter",
        "notification",
        "notifications",
        "noreply",
        "no-reply",
        "promo",
        "service",
        "services",
        "alerts",
        "updates",
    ]
    .contains(&first_label)
        || first_label.ends_with("-mail")
        || ["readwise.io", "substack.com", "paypal.com"]
            .iter()
            .any(|platform| domain == *platform || domain.ends_with(&format!(".{platform}")))
}

fn email_thread_snapshot(
    ctx: &ConnectorCtx,
    threads: &[FetchedThread],
) -> Result<(Vec<ThreadEvidence>, Vec<ParticipantThread>)> {
    let account = normalize_email(&ctx.account);
    let mut thread_rows = Vec::with_capacity(threads.len());
    let mut associations = Vec::new();
    for thread in threads {
        let dated_messages = thread
            .messages
            .iter()
            .map(|message| {
                message_datetime(&message.parsed)
                    .with_context(|| {
                        format!("Gmail thread {} has an invalid message date", thread.id)
                    })
                    .map(|timestamp| (message, timestamp))
            })
            .collect::<Result<Vec<_>>>()?;
        let occurred_from = dated_messages
            .iter()
            .map(|(_, timestamp)| *timestamp)
            .min()
            .context("Gmail thread has no messages")?;
        let occurred_to = dated_messages
            .iter()
            .map(|(_, timestamp)| *timestamp)
            .max()
            .context("Gmail thread has no messages")?;
        thread_rows.push(ThreadEvidence {
            thread_id: thread.id.clone(),
            occurred_from,
            occurred_to,
            body_text: render_thread_body_text(thread),
            href: Some(gmail_thread_url(&ctx.account, &thread.id)),
        });

        let mut participant_last: BTreeMap<String, DateTime<Utc>> = BTreeMap::new();
        for (message, timestamp) in dated_messages {
            for email in message_external_participants(&message.parsed, &account) {
                participant_last
                    .entry(email)
                    .and_modify(|current| {
                        if timestamp > *current {
                            *current = timestamp;
                        }
                    })
                    .or_insert(timestamp);
            }
        }
        for (participant, last_interaction) in participant_last {
            let user_sent = thread_user_sent_count(thread, &account, &participant);
            let user_replied = thread_user_reply_count(thread, &account, &participant);
            associations.push(ParticipantThread {
                participant,
                thread_id: thread.id.clone(),
                last_interaction,
                sampling_score: Some(correspondent_weight(user_replied, user_sent, 1)),
            });
        }
    }
    Ok((thread_rows, associations))
}

fn message_external_participants(message: &GmailMessage, account: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    ["from", "to", "cc", "bcc"]
        .into_iter()
        .flat_map(|key| message_participants_for_header(message, key))
        .filter_map(|person| person.email.map(|email| normalize_email(&email)))
        .filter(|email| email.contains('@') && email != account && seen.insert(email.clone()))
        .collect()
}

fn render_thread_body_text(thread: &FetchedThread) -> String {
    thread
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let timestamp = message_datetime(&message.parsed)
                .map(|value| value.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "time unavailable".to_string());
            let body = message.parsed.body.trim();
            format!("----- Message {} · {timestamp} -----\n\n{body}", index + 1)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn message_participants(message: &GmailMessage) -> Vec<PersonIdentity> {
    ["from", "to", "cc", "bcc"]
        .into_iter()
        .flat_map(|key| message_participants_for_header(message, key))
        .collect()
}

fn message_participants_for_header(message: &GmailMessage, key: &str) -> Vec<PersonIdentity> {
    header(message, key)
        .map(parse_address_list)
        .unwrap_or_default()
}

fn parse_address_list(value: &str) -> Vec<PersonIdentity> {
    split_addresses(value)
        .into_iter()
        .filter_map(|raw| {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            if let (Some(open), Some(close)) = (raw.rfind('<'), raw.rfind('>')) {
                if open < close {
                    let email = normalize_email(&raw[open + 1..close]);
                    if email.contains('@') {
                        let display = raw[..open].trim().trim_matches('"').trim();
                        return Some(PersonIdentity {
                            display_name: if display.is_empty() {
                                email.clone()
                            } else {
                                display.to_string()
                            },
                            email: Some(email),
                            aliases: Vec::new(),
                            ambiguous: false,
                            wikilink: None,
                        });
                    }
                }
            }
            let candidate = raw.trim_matches('"').trim();
            if candidate.contains('@') && !candidate.contains(char::is_whitespace) {
                let email = normalize_email(candidate);
                return Some(PersonIdentity {
                    display_name: email.clone(),
                    email: Some(email),
                    aliases: Vec::new(),
                    ambiguous: false,
                    wikilink: None,
                });
            }
            Some(PersonIdentity {
                display_name: candidate.to_string(),
                email: None,
                aliases: Vec::new(),
                ambiguous: true,
                wikilink: None,
            })
        })
        .collect()
}

fn split_addresses(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in value.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                current.push(ch);
            }
            ',' if !quoted => {
                values.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    values.push(current);
    values
}

fn message_datetime(message: &GmailMessage) -> Option<DateTime<Utc>> {
    let millis = message
        .internal_date
        .as_i64()
        .or_else(|| message.internal_date.as_str()?.parse().ok());
    if let Some(millis) = millis {
        if let Some(value) = Utc.timestamp_millis_opt(millis).single() {
            return Some(value);
        }
    }
    header(message, "date")
        .and_then(|value| DateTime::parse_from_rfc2822(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn header<'a>(message: &'a GmailMessage, key: &str) -> Option<&'a str> {
    message
        .headers
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

#[cfg(test)]
fn retry_quota_errors<T, F, S>(salt: &str, mut operation: F, mut sleep: S) -> Result<T>
where
    F: FnMut() -> Result<T>,
    S: FnMut(Duration),
{
    for attempt in 0..=MAX_QUOTA_RETRIES {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) if is_quota_error(&error) && attempt < MAX_QUOTA_RETRIES => {
                sleep(quota_retry_delay(salt, attempt));
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded quota retry loop always returns")
}

#[cfg(test)]
fn is_quota_error(error: &anyhow::Error) -> bool {
    let detail = format!("{error:#}").to_ascii_lowercase();
    detail.contains("429")
        || detail.contains("rate limit")
        || detail.contains("ratelimit")
        || detail.contains("quota")
        || detail.contains("resource_exhausted")
}

#[cfg(test)]
fn quota_retry_delay(salt: &str, attempt: usize) -> Duration {
    let exponential_ms = 1_000_u64.saturating_mul(1_u64 << attempt.min(5));
    let hash = salt.bytes().fold(attempt as u64 + 17, |state, byte| {
        state
            .wrapping_mul(1_099_511_628_211)
            .wrapping_add(byte as u64)
    });
    Duration::from_millis(exponential_ms.min(32_000) + (hash % 251))
}

fn compare_decimal_ids(left: &str, right: &str) -> std::cmp::Ordering {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

fn validate_email_ctx(ctx: &ConnectorCtx) -> Result<()> {
    if ctx.connector_id != EMAIL_CONNECTOR_ID {
        bail!("email connector requires connector_id=email");
    }
    if !normalize_email(&ctx.account).contains('@') {
        bail!("email connector requires an explicit account email");
    }
    Ok(())
}

fn normalize_email(value: &str) -> String {
    let normalized = value
        .trim()
        .trim_matches('<')
        .trim_matches('>')
        .to_lowercase();
    let Some((local, domain)) = normalized.rsplit_once('@') else {
        return normalized;
    };
    if matches!(domain, "gmail.com" | "googlemail.com") {
        let canonical_local = local.split_once('+').map_or(local, |(base, _)| base);
        return format!("{}@gmail.com", canonical_local.replace('.', ""));
    }
    normalized
}

fn normalize_domain(value: &str) -> String {
    value.trim().trim_start_matches('@').to_lowercase()
}

fn is_public_mail_domain(value: &str) -> bool {
    let domain = normalize_domain(value);
    matches!(
        domain.as_str(),
        "gmail.com"
            | "googlemail.com"
            | "outlook.com"
            | "hotmail.com"
            | "live.com"
            | "icloud.com"
            | "me.com"
            | "mac.com"
            | "proton.me"
            | "protonmail.com"
            | "aol.com"
            | "hey.com"
            | "fastmail.com"
            | "pm.me"
    ) || domain == "yahoo.com"
        || domain.starts_with("yahoo.")
}

fn email_domain(email: &str) -> Option<&str> {
    email.rsplit_once('@').map(|(_, domain)| domain)
}

fn gmail_thread_url(account: &str, thread_id: &str) -> String {
    format!(
        "https://mail.google.com/mail/?authuser={}#all/{}",
        percent_encode(account),
        percent_encode(thread_id)
    )
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn message(
        id: &str,
        thread_id: &str,
        from: &str,
        to: &str,
        subject: &str,
        list_unsubscribe: bool,
    ) -> FetchedMessage {
        let mut headers = BTreeMap::from([
            ("from".to_string(), from.to_string()),
            ("to".to_string(), to.to_string()),
            ("subject".to_string(), subject.to_string()),
        ]);
        if list_unsubscribe {
            headers.insert(
                "list-unsubscribe".to_string(),
                "<https://example.test/unsubscribe>".to_string(),
            );
        }
        FetchedMessage {
            parsed: GmailMessage {
                id: id.to_string(),
                thread_id: thread_id.to_string(),
                internal_date: serde_json::json!(1_787_220_300_000_i64),
                headers,
                body: String::new(),
            },
            raw: serde_json::Value::Null,
        }
    }

    fn thread(id: &str, messages: Vec<FetchedMessage>) -> FetchedThread {
        FetchedThread {
            id: id.to_string(),
            messages,
            history_id: None,
        }
    }

    #[derive(Debug, Clone)]
    struct NullGmailTransport;

    impl GmailThreadTransport for NullGmailTransport {
        fn search_page(
            &self,
            _ctx: &ConnectorCtx,
            _query: &str,
            _max_results: u64,
            _page_token: Option<&str>,
        ) -> Result<GmailSearchPage> {
            bail!("NullGmailTransport cannot fetch search pages")
        }

        fn fetch_thread(&self, _ctx: &ConnectorCtx, _thread_id: &str) -> Result<FetchedThread> {
            bail!("NullGmailTransport cannot fetch threads")
        }
    }

    #[test]
    fn thread_messages_render_neutral_plaintext_boundaries() {
        let mut first = message(
            "m-1",
            "three-person-thread",
            "Alice <ALICE@EXAMPLE.COM>",
            "Bob <bob@example.com>",
            "Planning",
            false,
        );
        first.parsed.body = "Alice opens the plan.".into();

        let mut second = message(
            "m-2",
            "three-person-thread",
            "Bob <BOB@example.com>",
            "Alice <alice@example.com>",
            "Re: Planning",
            false,
        );
        second.parsed.internal_date = serde_json::json!(1_787_220_360_000_i64);
        second.parsed.body = "Bob confirms the boundary.".into();

        let body = render_thread_body_text(&thread("three-person-thread", vec![first, second]));
        assert!(body.contains("Alice opens the plan."));
        assert!(body.contains("Bob confirms the boundary."));
        assert!(!body.contains("alice@example.com"));
        assert_eq!(body.matches("----- Message ").count(), 2);
        assert!(!body.contains("alice@example.com"));
        assert!(!body.contains("From:"));
        assert!(!body.contains("To:"));
        assert!(!body.contains("m-1"));
    }

    fn gmail_survey_thread(id: &str, sender: &str) -> FetchedThread {
        parse_gmail_thread_from_value(
            serde_json::json!({
                "thread": {
                    "id": id,
                    "messages": [{
                        "id": format!("message-{id}"),
                        "threadId": id,
                        "internalDate": "1787220300000",
                        "headers": {
                            "from": format!("Account Service {id} <{sender}>"),
                            "to": "Owner <owner@example.com>",
                            "subject": "Account update",
                            "date": "Thu, 20 Aug 2026 10:05:00 +0000"
                        },
                        "body": "A normal Gmail survey-time message without list metadata."
                    }]
                }
            }),
            id,
        )
        .unwrap()
    }

    #[test]
    fn gmail_fixture_preserves_headers_used_for_owner_authorship_detection() {
        let raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/email/gmail-thread-client.json"
        ))
        .unwrap();
        let thread = parse_gmail_thread_from_value(raw, "t-client").unwrap();
        assert_eq!(thread.history_id.as_deref(), Some("41003"));
        let owner_message = thread
            .messages
            .iter()
            .find(|message| message.parsed.id == "m-owner-reply")
            .expect("fixture must retain the owner-authored message");

        for required in ["from", "to", "date", "in-reply-to", "references"] {
            assert!(
                header(&owner_message.parsed, required).is_some(),
                "survey fetch fixture must contain the {required} header read by detection"
            );
        }
        assert!(message_from_account(
            &owner_message.parsed,
            "owner@example.com"
        ));
        let report = survey_threads(
            &ConnectorCtx {
                vault_root: PathBuf::new(),
                connector_id: EMAIL_CONNECTOR_ID.to_string(),
                account: "owner@example.com".to_string(),
                command_path: None,
            },
            &[thread],
        );
        let evidence = &report.proposal_evidence["person:alice@acme.test"];
        assert!(evidence["user_replied"].as_u64().unwrap() > 0);
        assert!(evidence["user_sent"].as_u64().unwrap() > 0);
    }

    #[test]
    fn quota_errors_back_off_exponentially_with_bounded_jitter() {
        let mut attempts = 0;
        let mut delays = Vec::new();
        let value = retry_quota_errors(
            "thread-42",
            || {
                attempts += 1;
                if attempts < 4 {
                    anyhow::bail!("HTTP 429 quota exceeded")
                }
                Ok("fetched")
            },
            |delay| delays.push(delay),
        )
        .unwrap();
        assert_eq!(value, "fetched");
        assert_eq!(attempts, 4);
        assert_eq!(delays.len(), 3);
        for (attempt, delay) in delays.into_iter().enumerate() {
            let base = 1_000_u128 << attempt;
            assert!(delay.as_millis() >= base);
            assert!(delay.as_millis() <= base + 250);
        }
        assert_eq!(THREAD_FETCH_WAVE_DELAY, Duration::from_millis(3_200));
    }

    #[test]
    fn default_selector_builds_documented_query_and_page_size() {
        let connector = GoogleEmailConnector::new(
            GmailCollectionSelector::default_declaration(),
            NullGmailTransport,
        );
        let query = connector.search_query();
        assert_eq!(query, "newer_than:365d -in:spam -in:trash");
        assert_eq!(SEARCH_PAGE_SIZE, 100);
    }

    #[test]
    fn gmail_owner_aliases_are_excluded_and_correspondent_aliases_merge() {
        let threads = vec![thread(
            "gmail-aliases",
            vec![
                message(
                    "gmail-alias-in",
                    "gmail-aliases",
                    "Client <A.Lice+intro@GoogleMail.com>",
                    concat!("Owner <owner.alias", "@gmail.com>"),
                    "Hello",
                    false,
                ),
                message(
                    "gmail-alias-out",
                    "gmail-aliases",
                    concat!("Owner <OWNER.ALIAS+sent", "@GoogleMail.com>"),
                    "Client <alice@gmail.com>",
                    "Re: Hello",
                    false,
                ),
            ],
        )];
        let report = survey_threads(
            &ConnectorCtx {
                vault_root: PathBuf::new(),
                connector_id: EMAIL_CONNECTOR_ID.to_string(),
                account: concat!("owner.alias", "@gmail.com").to_string(),
                command_path: None,
            },
            &threads,
        );

        assert!(report
            .proposed_include
            .contains(&"person:alice@gmail.com".to_string()));
        assert!(!report
            .proposed_include
            .iter()
            .any(|proposal| proposal.contains("owneralias")));
        assert_eq!(
            report
                .proposed_include
                .iter()
                .filter(|proposal| proposal.starts_with("person:alice"))
                .count(),
            1
        );
        assert!(
            report.proposal_evidence["person:alice@gmail.com"]["user_sent"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(
            report.proposal_evidence["person:alice@gmail.com"]["user_replied"]
                .as_u64()
                .unwrap()
                > 0
        );
        assert!(!report
            .proposed_include
            .iter()
            .any(|proposal| proposal == "domain:gmail.com"));
        assert!(!report.orgs.iter().any(|org| org.label == "gmail.com"));
    }

    #[test]
    fn public_mail_domains_never_become_account_proposals() {
        let owner = "Owner <owner@agency.test>";
        let domains = [
            "gmail.com",
            "googlemail.com",
            "outlook.com",
            "hotmail.com",
            "live.com",
            "yahoo.co.uk",
            "icloud.com",
            "me.com",
            "proton.me",
            "protonmail.com",
            "aol.com",
        ];
        let threads = domains
            .iter()
            .enumerate()
            .map(|(index, domain)| {
                thread(
                    &format!("public-{index}"),
                    vec![message(
                        &format!("public-{index}-message"),
                        &format!("public-{index}"),
                        &format!("Person {index} <person@{domain}>"),
                        owner,
                        "Hello",
                        false,
                    )],
                )
            })
            .collect::<Vec<_>>();
        let report = survey_threads(
            &ConnectorCtx {
                vault_root: PathBuf::new(),
                connector_id: EMAIL_CONNECTOR_ID.to_string(),
                account: "owner@agency.test".to_string(),
                command_path: None,
            },
            &threads,
        );
        assert!(report
            .proposed_include
            .iter()
            .any(|proposal| proposal == "person:person@gmail.com"));
        assert!(!report
            .proposed_include
            .iter()
            .any(|proposal| proposal.starts_with("domain:")));
        assert!(report.orgs.is_empty());
    }

    #[test]
    fn selector_composes_backfill_and_query_without_approval_scope() {
        let connector = GoogleEmailConnector::new(
            GmailCollectionSelector {
                query: "label:important -in:spam".into(),
                backfill_days: 30,
            },
            NullGmailTransport,
        );
        assert_eq!(
            connector.search_query(),
            "newer_than:30d label:important -in:spam"
        );
    }

    #[test]
    fn survey_ranks_reply_then_sent_then_mixed_then_automated() {
        let owner = "Owner <owner@example.com>";
        let threads = vec![
            thread(
                "reply",
                vec![
                    message(
                        "reply-in",
                        "reply",
                        "Replied Human <reply@human.test>",
                        owner,
                        "Question",
                        false,
                    ),
                    message(
                        "reply-out",
                        "reply",
                        owner,
                        "Replied Human <reply@human.test>",
                        "Re: Question",
                        false,
                    ),
                    message(
                        "reply-out-again",
                        "reply",
                        owner,
                        "Replied Human <reply@human.test>",
                        "Re: Question",
                        false,
                    ),
                ],
            ),
            thread(
                "sent",
                vec![message(
                    "sent-out",
                    "sent",
                    owner,
                    "Sent Human <sent@sent.test>",
                    "Introduction",
                    false,
                )],
            ),
            thread(
                "mixed-auto",
                vec![message(
                    "mixed-auto-in",
                    "mixed-auto",
                    "Notifications <notifications@mixed.test>",
                    owner,
                    "Daily digest",
                    true,
                )],
            ),
            thread(
                "mixed-human",
                vec![message(
                    "mixed-human-in",
                    "mixed-human",
                    "Notifications <notifications@mixed.test>",
                    owner,
                    "A personal note",
                    false,
                )],
            ),
            thread(
                "automated",
                vec![message(
                    "automated-in",
                    "automated",
                    "Newsletter <newsletter@noise.test>",
                    owner,
                    "Daily digest",
                    true,
                )],
            ),
        ];
        let report = survey_threads(
            &ConnectorCtx {
                vault_root: PathBuf::new(),
                connector_id: EMAIL_CONNECTOR_ID.to_string(),
                account: "owner@example.com".to_string(),
                command_path: None,
            },
            &threads,
        );
        let rank = |proposal: &str| {
            report
                .proposed_include
                .iter()
                .position(|value| value == proposal)
                .unwrap()
        };
        assert!(rank("person:reply@human.test") < rank("person:sent@sent.test"));
        assert!(rank("person:sent@sent.test") < rank("person:notifications@mixed.test"));
        assert!(rank("person:notifications@mixed.test") < rank("person:newsletter@noise.test"));
        assert!(report
            .proposal_flags
            .contains_key("person:notifications@mixed.test"));
        assert!(report
            .proposal_flags
            .contains_key("person:newsletter@noise.test"));
        assert!(!report
            .proposal_flags
            .contains_key("person:reply@human.test"));
        assert_eq!(
            report.proposal_evidence["person:reply@human.test"]["user_replied"],
            2
        );
        assert_eq!(
            report.proposal_evidence["person:reply@human.test"]["user_sent"],
            2
        );
        assert_eq!(
            report.proposal_evidence["person:reply@human.test"]["thread_count"],
            1
        );
        assert_eq!(
            report.proposal_evidence["person:notifications@mixed.test"]["threads"],
            2
        );
        assert_eq!(report.proposal_evidence["domain:mixed.test"]["threads"], 2);
    }

    #[test]
    fn snapshot_derives_one_role_blind_edge_per_external_participant() {
        let temp = tempfile::tempdir().unwrap();
        let account = "owner.round9@gmail.com";
        let ctx = ConnectorCtx {
            vault_root: temp.path().to_path_buf(),
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            account: account.to_string(),
            command_path: None,
        };
        let fetched = parse_gmail_thread_from_value(
            serde_json::json!({
                "thread": {
                    "id": "shared-thread",
                    "messages": [
                        {
                            "id": "shared-in",
                            "threadId": "shared-thread",
                            "internalDate": 1_787_220_300_000_i64,
                            "headers": {
                                "from": "DK Kim <dk@matching.test>",
                                "to": "Owner <owner.round9@gmail.com>, Bob Zhao <bob@matching.test>, Carter <carter@other.test>",
                                "cc": "Carter <carter@other.test>",
                                "bcc": "Bea <bea@hidden.test>",
                                "subject": "Complete Matching Survey",
                                "date": "Thu, 20 Aug 2026 10:05:00 +0000",
                                "message_id": "<shared-in@matching.test>"
                            },
                            "body": "shared participant body"
                        },
                        {
                            "id": "shared-reply",
                            "threadId": "shared-thread",
                            "internalDate": 1_787_221_200_000_i64,
                            "headers": {
                                "from": "Owner <owner.round9@gmail.com>",
                                "to": "DK Kim <dk@matching.test>, Bob Zhao <bob@matching.test>",
                                "subject": "Re: Complete Matching Survey",
                                "date": "Thu, 20 Aug 2026 10:20:00 +0000",
                                "in_reply_to": "<shared-in@matching.test>",
                                "message_id": "<shared-reply@gmail.com>"
                            },
                            "body": "owner reply"
                        }
                    ]
                }
            }),
            "shared-thread",
        )
        .unwrap();
        let store = IntegrationsStore::open(temp.path()).unwrap();
        let (thread_rows, associations) =
            email_thread_snapshot(&ctx, std::slice::from_ref(&fetched)).unwrap();
        store
            .replace_email_thread_snapshot(&ctx, thread_rows, associations)
            .unwrap();

        let selection = automatic_correspondent_entity_selection(temp.path(), account).unwrap();
        let identities = selection
            .entities
            .iter()
            .map(|entity| entity.identity_key.as_str())
            .collect::<BTreeSet<_>>();
        for expected in [
            "bea@hidden.test",
            "bob@matching.test",
            "carter@other.test",
            "dk@matching.test",
        ] {
            assert!(
                identities.contains(expected),
                "shared-thread identity missing: {expected}"
            );
        }
        assert!(selection.correspondents_considered >= 3);
        let participants = store
            .participant_threads(&ctx)
            .unwrap()
            .into_iter()
            .filter(|edge| edge.thread_id == "shared-thread")
            .map(|edge| edge.participant)
            .collect::<BTreeSet<_>>();
        assert_eq!(participants.len(), 4, "duplicate To/CC identities collapse");
        for expected in [
            "bea@hidden.test",
            "bob@matching.test",
            "carter@other.test",
            "dk@matching.test",
        ] {
            assert!(participants.contains(expected));
        }
        assert!(!participants.contains(account));
    }

    #[test]
    fn curation_exclusions_do_not_depend_on_connector_cache_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let account = "owner.round9@gmail.com";
        let ctx = ConnectorCtx {
            vault_root: temp.path().to_path_buf(),
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            account: account.to_string(),
            command_path: None,
        };
        let store = IntegrationsStore::open(temp.path()).unwrap();
        store
            .ingest_raw_connector_items(
                &ctx,
                vec![RawItemDraft {
                    source_id: "cache-only".to_string(),
                    payload: serde_json::json!({"connector_cache": "not evidence"}),
                }],
            )
            .unwrap();
        let report = CurationObservation {
            connector_id: EMAIL_CONNECTOR_ID.to_string(),
            account: account.to_string(),
            item_counts: KindCounts::default(),
            occurred_from: None,
            occurred_to: None,
            observation_window: None,
            observed_range: None,
            sample_observed_ranges: BTreeMap::new(),
            observation_call_count: None,
            detected_accounts: vec![account.to_string()],
            inferred_people: Vec::new(),
            inferred_orgs: Vec::new(),
            proposed_include: vec![
                "person:ci_activity@noreply.github.com".to_string(),
                "domain:noreply.github.com".to_string(),
                "person:human@gmail.com".to_string(),
            ],
            proposed_exclude: Vec::new(),
            proposal_flags: BTreeMap::from([
                (
                    "person:ci_activity@noreply.github.com".to_string(),
                    vec!["likely-automated".to_string()],
                ),
                (
                    "domain:noreply.github.com".to_string(),
                    vec!["likely-automated".to_string()],
                ),
            ]),
            proposal_evidence: BTreeMap::new(),
            decisions_required: Vec::new(),
        };
        store
            .replace_email_thread_snapshot_with_policy_report(
                &ctx,
                Vec::new(),
                Vec::new(),
                &report,
                None,
                "cache-only",
                Utc::now(),
                None,
                None,
            )
            .unwrap();

        let selection = automatic_correspondent_entity_selection(temp.path(), account).unwrap();
        assert!(selection.entities.is_empty());
        assert_eq!(selection.correspondents_considered, 0);
        for expected in [
            "owner.round9@gmail.com",
            "ownerround9@gmail.com",
            "ci_activity@noreply.github.com",
            "noreply.github.com",
            "gmail.com",
        ] {
            assert!(
                selection
                    .excluded_links
                    .iter()
                    .any(|value| value == expected),
                "independent exclusion missing: {expected}"
            );
        }
    }

    #[test]
    fn realistic_gmail_threads_flag_named_automated_sender_domains_without_list_headers() {
        let senders = [
            "service@academia-mail.com",
            "pharmacy@email.pharmacy.amazon.com",
            "tickets@mail.stubhub.com",
            "alerts@notification.capitalone.com",
            "deals@promo.newegg.com",
            "digest@readwise.io",
            "post@substack.com",
            "service@paypal.com",
            "ci@noreply.github.com",
            "statements@email.discover.com",
            "discover@services.discover.com",
        ];
        let threads = senders
            .iter()
            .enumerate()
            .map(|(index, sender)| gmail_survey_thread(&format!("automated-{index}"), sender))
            .collect::<Vec<_>>();
        let report = survey_threads(
            &ConnectorCtx {
                vault_root: PathBuf::new(),
                connector_id: EMAIL_CONNECTOR_ID.to_string(),
                account: "owner@example.com".to_string(),
                command_path: None,
            },
            &threads,
        );

        for sender in senders {
            let domain = email_domain(sender).unwrap();
            for proposal in [format!("person:{sender}"), format!("domain:{domain}")] {
                assert_eq!(
                    report.proposal_flags.get(&proposal),
                    Some(&vec!["likely-automated".to_string()]),
                    "missing pattern-only flag for {proposal}"
                );
            }
        }
    }

    #[test]
    fn sender_pattern_flags_without_metadata_but_a_real_reply_vetoes() {
        let owner = "Owner <owner@example.com>";
        let threads = vec![
            gmail_survey_thread("promo", "promo@ordinary.example"),
            thread(
                "replied-noreply",
                vec![
                    message(
                        "replied-noreply-in",
                        "replied-noreply",
                        "Service <noreply@replied.example>",
                        owner,
                        "Account update",
                        false,
                    ),
                    message(
                        "replied-noreply-out",
                        "replied-noreply",
                        owner,
                        "Service <noreply@replied.example>",
                        "Re: Account update",
                        false,
                    ),
                ],
            ),
        ];
        let report = survey_threads(
            &ConnectorCtx {
                vault_root: PathBuf::new(),
                connector_id: EMAIL_CONNECTOR_ID.to_string(),
                account: "owner@example.com".to_string(),
                command_path: None,
            },
            &threads,
        );

        assert!(report
            .proposal_flags
            .contains_key("person:promo@ordinary.example"));
        assert!(!report
            .proposal_flags
            .contains_key("person:noreply@replied.example"));
        assert_eq!(
            report.proposed_include.first().map(String::as_str),
            Some("domain:replied.example")
        );
    }
}
