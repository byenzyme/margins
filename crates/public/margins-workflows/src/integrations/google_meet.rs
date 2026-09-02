//! Google Meet transcript connector.
//!
//! Google Docs remain the authoritative transcript body. Meet v2 metadata is
//! joined only through `Transcript.docsDestination.document`; participant
//! names are treated as evidence and gain an email/wikilink only when an exact
//! signed-in-user or full-name calendar attendee match is unique.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::{
    Connector, ConnectorCtx, ExternalDocumentDelta, ExternalDocumentEvidence,
    ExternalDocumentParticipant, HealthReport, HealthStatus, IntegrationsStore, PersonIdentity,
    RawItemDraft, ReconcileResult,
};
use crate::workspace::WorkspaceMutationError;

pub const GOOGLE_MEET_CONNECTOR_ID: &str = "google_meet";
pub const GOOGLE_MEET_MATERIALIZATION_FINGERPRINT: &str =
    "google-meet-collection-v1:all-meet-recordings";

/// One Google Doc discovered under Drive's Meet Recordings folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveTranscriptDocument {
    pub id: String,
    pub name: String,
    pub created_time: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_time: Option<DateTime<Utc>>,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_view_link: Option<String>,
    #[serde(default)]
    pub raw: Value,
}

/// Meet v2 transcript metadata. `document` has the API form `documents/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetTranscriptMetadata {
    pub name: String,
    pub document: String,
    pub start_time: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<DateTime<Utc>>,
}

/// Participant evidence from Meet v2. The API itself does not expose email.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetParticipantEvidence {
    pub resource_name: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_user_id: Option<String>,
}

/// Calendar/People evidence used to add an email to a Meet participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarAttendeeEvidence {
    pub display_name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_user_id: Option<String>,
    /// Human-readable source evidence, normally a Calendar event id/link.
    pub evidence: String,
}

/// Metadata for one held conference, normally assembled from Meet v2 plus an
/// exactly correlated Calendar event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetConferenceRecord {
    pub name: String,
    pub start_time: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub transcripts: Vec<MeetTranscriptMetadata>,
    #[serde(default)]
    pub participants: Vec<MeetParticipantEvidence>,
    #[serde(default)]
    pub calendar_attendees: Vec<CalendarAttendeeEvidence>,
    #[serde(default)]
    pub raw: Value,
}

/// Read-only source snapshot returned by a swappable transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleMeetSnapshot {
    pub meet_recordings_folder_id: String,
    #[serde(default)]
    pub documents: Vec<DriveTranscriptDocument>,
    #[serde(default)]
    pub conferences: Vec<MeetConferenceRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<Value>,
}

/// Swappable Drive/Docs + Meet boundary. CI implementations load recorded JSON;
/// production uses the native Google REST driver.
pub trait GoogleMeetTransport {
    fn fetch(&self, account: &str, cursor: Option<&Value>) -> Result<GoogleMeetSnapshot>;

    fn health(&self, _account: &str) -> Result<HealthStatus> {
        Ok(HealthStatus::Fresh)
    }
}

/// Parse a recorded/assembled Meet REST v2 bundle. Meet's list resources are
/// separate HTTP surfaces; transports may bundle their responses by conference
/// using the native camelCase resource shapes plus an optional
/// `calendarAttendees` correlation sidecar.
pub fn parse_meet_v2_bundle(json: &str) -> Result<Vec<MeetConferenceRecord>> {
    let root: Value = serde_json::from_str(json).context("failed to parse Meet v2 JSON")?;
    let records = root
        .get("conferenceRecords")
        .and_then(Value::as_array)
        .context("Meet v2 bundle contains no conferenceRecords array")?;
    records
        .iter()
        .map(|record| {
            let name = json_string(record, &["name"]).context("conference record has no name")?;
            let start_time = parse_json_time(record, &["startTime"])
                .with_context(|| format!("conference record {name} has no startTime"))?;
            let end_time = parse_json_time(record, &["endTime"]);
            let transcripts = record
                .get("transcripts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|transcript| {
                    let transcript_name = json_string(transcript, &["name"])
                        .context("Meet transcript has no name")?;
                    let document = transcript
                        .pointer("/docsDestination/document")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .with_context(|| {
                            format!("Meet transcript {transcript_name} has no Docs destination")
                        })?;
                    let transcript_start = parse_json_time(transcript, &["startTime"])
                        .with_context(|| {
                            format!("Meet transcript {transcript_name} has no startTime")
                        })?;
                    Ok(MeetTranscriptMetadata {
                        name: transcript_name,
                        document,
                        start_time: transcript_start,
                        end_time: parse_json_time(transcript, &["endTime"]),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let participants = record
                .get("participants")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|participant| {
                    let user = participant
                        .get("signedinUser")
                        .or_else(|| participant.get("anonymousUser"))
                        .or_else(|| participant.get("phoneUser"))?;
                    Some(MeetParticipantEvidence {
                        resource_name: json_string(participant, &["name"])
                            .unwrap_or_else(|| "unknown-participant".to_string()),
                        display_name: json_string(user, &["displayName"])
                            .unwrap_or_else(|| "Unknown attendee".to_string()),
                        google_user_id: json_string(user, &["user"]),
                    })
                })
                .collect();
            let calendar_attendees = record
                .get("calendarAttendees")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|attendee| {
                    Some(CalendarAttendeeEvidence {
                        display_name: json_string(attendee, &["displayName"])?,
                        email: json_string(attendee, &["email"])?,
                        google_user_id: json_string(attendee, &["googleUserId"]),
                        evidence: json_string(attendee, &["evidence"])
                            .unwrap_or_else(|| "calendar attendee".to_string()),
                    })
                })
                .collect();
            Ok(MeetConferenceRecord {
                name,
                start_time,
                end_time,
                transcripts,
                participants,
                calendar_attendees,
                raw: record.clone(),
            })
        })
        .collect()
}

fn json_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn parse_json_time(value: &Value, keys: &[&str]) -> Option<DateTime<Utc>> {
    json_string(value, keys).and_then(|value| {
        DateTime::parse_from_rfc3339(&value)
            .ok()
            .map(|time| time.with_timezone(&Utc))
    })
}

/// Portable connector over a swappable transport.
pub struct GoogleMeetConnector<T> {
    transport: T,
}

impl<T> GoogleMeetConnector<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: GoogleMeetTransport> Connector for GoogleMeetConnector<T> {
    fn reconcile(
        &self,
        ctx: &ConnectorCtx,
        expected_workspace_revision: Option<&str>,
    ) -> Result<ReconcileResult> {
        validate_ctx(ctx)?;
        let store = IntegrationsStore::open(&ctx.vault_root)?;
        let result = (|| {
            // Meet account bindings mean the complete discoverable Meet
            // Recordings corpus. Full snapshots make moved/deleted Docs
            // observable and keep approval out of corpus membership.
            let snapshot = self.transport.fetch(&ctx.account, None)?;
            let correlated = correlate_snapshot(&snapshot, &ctx.account);
            let mut documents = Vec::with_capacity(correlated.len());
            let mut participants = Vec::new();
            let mut raw_items = Vec::with_capacity(correlated.len());
            for item in correlated {
                let (document, document_participants, raw) = item.into_evidence();
                raw_items.push(raw);
                participants.extend(document_participants);
                documents.push(document);
            }
            store.apply_external_document_delta(
                ctx,
                ExternalDocumentDelta {
                    documents,
                    participants,
                    tombstone_source_ids: Vec::new(),
                    raw_items,
                    snapshot_scope: None,
                    complete_snapshot: true,
                    materialization_fingerprint: GOOGLE_MEET_MATERIALIZATION_FINGERPRINT
                        .to_string(),
                    next_cursor: None,
                },
                expected_workspace_revision,
            )
        })();
        if let Err(error) = &result {
            if error.downcast_ref::<WorkspaceMutationError>().is_none() {
                let _ = store.record_failed_reconcile(ctx, &error.to_string());
            }
        }
        result
    }

    fn health(&self, ctx: &ConnectorCtx) -> Result<HealthReport> {
        validate_ctx(ctx)?;
        let store = IntegrationsStore::open(&ctx.vault_root)?;
        match self.transport.health(&ctx.account)? {
            HealthStatus::Fresh => store
                .health_report_for_materialization(ctx, GOOGLE_MEET_MATERIALIZATION_FINGERPRINT),
            status => store.update_health(ctx, status, Some("Google Meet transport check")),
        }
    }
}

fn validate_ctx(ctx: &ConnectorCtx) -> Result<()> {
    if ctx.connector_id != GOOGLE_MEET_CONNECTOR_ID {
        bail!("Google Meet connector requires connector_id={GOOGLE_MEET_CONNECTOR_ID}");
    }
    if ctx.account.trim().is_empty() {
        bail!("Google Meet connector account is empty");
    }
    Ok(())
}

struct CorrelatedTranscript {
    document: DriveTranscriptDocument,
    occurred_at: DateTime<Utc>,
    conference_name: Option<String>,
    transcript_name: Option<String>,
    people: Vec<PersonIdentity>,
    orgs: Vec<String>,
    flags: Vec<String>,
    raw: Value,
}

impl CorrelatedTranscript {
    fn into_evidence(
        self,
    ) -> (
        ExternalDocumentEvidence,
        Vec<ExternalDocumentParticipant>,
        RawItemDraft,
    ) {
        let source_id = self.document.id.clone();
        let participants = self
            .people
            .iter()
            .enumerate()
            .map(|(position, person)| ExternalDocumentParticipant {
                source_id: source_id.clone(),
                participant_key: person.email.as_ref().map_or_else(
                    || format!("position:{position}"),
                    |email| format!("email:{}", email.to_ascii_lowercase()),
                ),
                position: u32::try_from(position).unwrap_or(u32::MAX),
                display_name: person.display_name.clone(),
                email: person.email.clone(),
                ambiguous: person.ambiguous,
            })
            .collect();
        let raw = RawItemDraft {
            source_id: source_id.clone(),
            payload: serde_json::json!({
                "drive": self.document.raw,
                "conference": self.raw,
            }),
        };
        let evidence = ExternalDocumentEvidence {
            source_id,
            occurred_at: self.occurred_at,
            title: self.document.name.clone(),
            body_text: format!("## Transcript\n\n{}", self.document.body.trim()),
            // Canonical Docs URLs open the source and do not depend on a
            // possibly absent Drive `webViewLink` field mask.
            href: Some(format!(
                "https://docs.google.com/document/d/{}/edit",
                self.document.id
            )),
            attributes: serde_json::json!({
                "conference_name": self.conference_name,
                "transcript_name": self.transcript_name,
                "identity_flags": self.flags,
                "organizations": self.orgs,
            }),
        };
        (evidence, participants, raw)
    }
}

fn correlate_snapshot(
    snapshot: &GoogleMeetSnapshot,
    source_account: &str,
) -> Vec<CorrelatedTranscript> {
    snapshot
        .documents
        .iter()
        .filter(|document| {
            document.name.to_ascii_lowercase().contains("transcript")
                || snapshot.conferences.iter().any(|conference| {
                    conference.transcripts.iter().any(|transcript| {
                        document_id(&transcript.document) == document.id
                    })
                })
        })
        .cloned()
        .map(|document| {
            let matches = snapshot
                .conferences
                .iter()
                .flat_map(|conference| {
                    conference
                        .transcripts
                        .iter()
                        .filter(|transcript| document_id(&transcript.document) == document.id)
                        .map(move |transcript| (conference, transcript))
                })
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [(conference, transcript)] => {
                    let (people, orgs, flags) = resolve_people(conference, source_account);
                    CorrelatedTranscript {
                        occurred_at: transcript.start_time,
                        conference_name: Some(conference.name.clone()),
                        transcript_name: Some(transcript.name.clone()),
                        people,
                        orgs,
                        flags,
                        raw: conference.raw.clone(),
                        document,
                    }
                }
                [] => CorrelatedTranscript {
                    occurred_at: document.created_time,
                    conference_name: None,
                    transcript_name: None,
                    people: Vec::new(),
                    orgs: Vec::new(),
                    flags: vec![format!(
                        "No exact Meet transcript metadata matched Drive document {}; attendee identity omitted",
                        document.id
                    )],
                    raw: Value::Null,
                    document,
                },
                many => {
                    let people = many
                        .iter()
                        .flat_map(|(conference, _)| conference.participants.iter())
                        .map(|person| PersonIdentity {
                            display_name: person.display_name.clone(),
                            email: None,
                            aliases: person.google_user_id.clone().into_iter().collect(),
                            ambiguous: true,
                            wikilink: None,
                        })
                        .collect::<Vec<_>>();
                    CorrelatedTranscript {
                        occurred_at: document.created_time,
                        conference_name: None,
                        transcript_name: None,
                        people: dedupe_people(people),
                        orgs: Vec::new(),
                        flags: vec![format!(
                            "Drive document {} matched {} Meet transcripts; all attendee identities remain evidence-only",
                            document.id,
                            many.len()
                        )],
                        raw: Value::Null,
                        document,
                    }
                }
            }
        })
        .collect()
}

fn resolve_people(
    conference: &MeetConferenceRecord,
    source_account: &str,
) -> (Vec<PersonIdentity>, Vec<String>, Vec<String>) {
    let mut people = Vec::new();
    let mut flags = Vec::new();
    for participant in &conference.participants {
        let by_user = participant.google_user_id.as_ref().map(|user| {
            conference
                .calendar_attendees
                .iter()
                .filter(|attendee| attendee.google_user_id.as_ref() == Some(user))
                .collect::<Vec<_>>()
        });
        let exact_name = conference
            .calendar_attendees
            .iter()
            .filter(|attendee| {
                normalize_name(&attendee.display_name) == normalize_name(&participant.display_name)
            })
            .collect::<Vec<_>>();
        let candidates = match by_user {
            Some(matches) if !matches.is_empty() => matches,
            _ => exact_name,
        };
        let unique_email = candidates
            .iter()
            .map(|attendee| attendee.email.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if candidates.len() == 1 && unique_email.len() == 1 {
            let email = candidates[0].email.to_ascii_lowercase();
            people.push(PersonIdentity {
                display_name: participant.display_name.clone(),
                email: Some(email),
                aliases: participant.google_user_id.clone().into_iter().collect(),
                ambiguous: false,
                wikilink: Some(participant.display_name.clone()),
            });
        } else {
            people.push(PersonIdentity {
                display_name: participant.display_name.clone(),
                email: None,
                aliases: participant.google_user_id.clone().into_iter().collect(),
                ambiguous: true,
                wikilink: None,
            });
            let same_first = conference
                .calendar_attendees
                .iter()
                .filter(|attendee| {
                    first_name(&attendee.display_name) == first_name(&participant.display_name)
                })
                .count();
            let reason = if candidates.len() > 1 || same_first > 1 {
                "ambiguous attendee identity"
            } else {
                "no exact email-bearing attendee evidence"
            };
            flags.push(format!(
                "{} ({reason}); retained as evidence-only",
                participant.display_name
            ));
        }
    }
    let people = dedupe_people(people);
    let source_domain = email_domain(source_account);
    let mut orgs = BTreeSet::new();
    for person in &people {
        if let Some(domain) = person.email.as_deref().and_then(email_domain) {
            if Some(domain) != source_domain && !is_public_mail_domain(domain) {
                orgs.insert(domain.to_string());
            }
        }
    }
    (people, orgs.into_iter().collect(), flags)
}

fn dedupe_people(people: Vec<PersonIdentity>) -> Vec<PersonIdentity> {
    let mut out = BTreeMap::new();
    for person in people {
        let key = person
            .email
            .clone()
            .unwrap_or_else(|| normalize_name(&person.display_name));
        out.entry(key).or_insert(person);
    }
    out.into_values().collect()
}

fn document_id(value: &str) -> &str {
    value.strip_prefix("documents/").unwrap_or(value)
}

fn normalize_name(value: &str) -> String {
    value
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn first_name(value: &str) -> String {
    value
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn email_domain(value: &str) -> Option<&str> {
    value.rsplit_once('@').map(|(_, domain)| domain)
}

fn is_public_mail_domain(domain: &str) -> bool {
    matches!(
        domain.to_ascii_lowercase().as_str(),
        "gmail.com" | "googlemail.com" | "outlook.com" | "hotmail.com" | "yahoo.com" | "icloud.com"
    )
}
