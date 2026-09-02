//! Portable Google Calendar connector with a swappable fetch transport.
//!
//! OAuth credentials, refresh, pagination, quotas and HTTP belong to the
//! injected driver. This module owns event materialization and ledger writes.

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::connector::Connector;
use super::store::IntegrationsStore;
use super::types::{
    CalendarEventAttendee, CalendarEventDelta, CalendarEventEvidence, ConnectorCtx, HealthReport,
    HealthStatus, RawItemDraft, ReconcileResult, SurveyRange,
};
use crate::workspace::{CalendarCollectionSelector, WorkspaceMutationError};

pub const GOOGLE_CALENDAR_CONNECTOR_ID: &str = "gcal";

/// Bounded occurrence window for the initial snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleCalendarScope {
    pub occurred_from: DateTime<Utc>,
    pub occurred_to: DateTime<Utc>,
}

impl GoogleCalendarScope {
    pub fn for_selector(selector: &CalendarCollectionSelector, now: DateTime<Utc>) -> Result<Self> {
        let today = now.date_naive();
        let occurred_from = today
            .checked_sub_signed(Duration::days(i64::from(selector.lookback_days)))
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .context("Calendar lookback boundary is outside the supported date range")?
            .and_utc();
        let occurred_to = today
            .checked_add_signed(Duration::days(i64::from(selector.lookahead_days) + 1))
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .context("Calendar lookahead boundary is outside the supported date range")?
            .and_utc()
            - Duration::milliseconds(1);
        Ok(Self {
            occurred_from,
            occurred_to,
        })
    }

    pub fn as_range(self) -> SurveyRange {
        SurveyRange {
            occurred_from: self.occurred_from,
            occurred_to: self.occurred_to,
        }
    }
}

/// Fetch request understood by any Google Calendar transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarFetchRequest {
    pub account: String,
    pub calendars: Vec<String>,
    pub scope: GoogleCalendarScope,
    /// Absent for the initial bounded snapshot; present for changed-event refreshes.
    pub updated_since: Option<DateTime<Utc>>,
    /// Per-calendar Google `nextSyncToken` values from the previous complete page walk.
    pub sync_tokens: BTreeMap<String, String>,
}

/// Raw attendee evidence supplied by Google Calendar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarAttendee {
    #[serde(default, alias = "displayName")]
    pub display_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub resource: bool,
    #[serde(default, rename = "self")]
    pub is_self: bool,
    #[serde(default, alias = "responseStatus")]
    pub response_status: Option<String>,
}

/// Loss-minimized event shape consumed from Google Calendar API JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    #[serde(default, alias = "calendarId", alias = "CalendarID")]
    pub calendar_id: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    pub start: Value,
    #[serde(default)]
    pub end: Option<Value>,
    #[serde(default)]
    pub updated: Option<String>,
    #[serde(default, alias = "htmlLink")]
    pub html_link: Option<String>,
    #[serde(default)]
    pub attendees: Vec<CalendarAttendee>,
    #[serde(default)]
    pub organizer: Option<CalendarAttendee>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarFetchBatch {
    pub events: Vec<CalendarEvent>,
    pub next_cursor: Value,
    pub complete_snapshot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarAuthState {
    Ready,
    NeedsAuth(String),
}

/// Provider-transport boundary used by the production native driver and fixtures.
pub trait CalendarTransport: Send + Sync {
    fn fetch(&self, request: &CalendarFetchRequest) -> Result<CalendarFetchBatch>;
    fn auth_state(&self, account: &str) -> Result<CalendarAuthState>;
}

#[cfg(test)]
fn parse_calendar_events_from_value(payload: Value) -> Result<Vec<CalendarEvent>> {
    let events = match payload {
        Value::Array(events) => events,
        Value::Object(mut object) => object
            .remove("events")
            .or_else(|| object.remove("items"))
            .and_then(|value| value.as_array().cloned())
            .context("Google Calendar response did not contain an events array")?,
        _ => bail!("Google Calendar response was not an object or array"),
    };
    events
        .into_iter()
        .map(|event| {
            serde_json::from_value(event).context("failed to decode Google Calendar event")
        })
        .collect()
}

/// Connector implementation shared by fixture and native transports.
pub struct GoogleCalendarConnector<T> {
    transport: T,
    selector: CalendarCollectionSelector,
    scope_anchor: DateTime<Utc>,
}

impl<T> GoogleCalendarConnector<T> {
    pub fn new(transport: T, selector: CalendarCollectionSelector) -> Self {
        Self::new_at(transport, selector, Utc::now())
    }

    pub fn new_at(
        transport: T,
        selector: CalendarCollectionSelector,
        scope_anchor: DateTime<Utc>,
    ) -> Self {
        Self {
            transport,
            selector,
            scope_anchor,
        }
    }

    pub fn with_default_selector(transport: T) -> Self {
        Self::new(transport, CalendarCollectionSelector::default_declaration())
    }

    fn scope(&self) -> Result<GoogleCalendarScope> {
        GoogleCalendarScope::for_selector(&self.selector, self.scope_anchor)
    }
}

impl<T: CalendarTransport> Connector for GoogleCalendarConnector<T> {
    fn reconcile(
        &self,
        ctx: &ConnectorCtx,
        expected_workspace_revision: Option<&str>,
    ) -> Result<ReconcileResult> {
        validate_ctx(ctx)?;
        self.selector.validate()?;
        let store = IntegrationsStore::open(&ctx.vault_root)?;
        let scope = self.scope()?;
        let scope_range = scope.as_range();
        let materialization_fingerprint = self.selector.materialization_fingerprint()?;
        let stored_cursor = store
            .load_cursor(&ctx.connector_id, &ctx.account)?
            .as_ref()
            .map(parse_cursor)
            .transpose()?;
        let complete_snapshot = stored_cursor.is_none()
            || store.materialization_fingerprint(ctx)?.as_deref()
                != Some(materialization_fingerprint.as_str())
            || store.calendar_materialization_scope(ctx)?.as_ref() != Some(&scope_range);
        let request = CalendarFetchRequest {
            account: ctx.account.clone(),
            // Account-wide accessible calendars are the collection. Survey
            // observations never narrow this transport request.
            calendars: Vec::new(),
            scope,
            updated_since: (!complete_snapshot)
                .then_some(
                    stored_cursor
                        .as_ref()
                        .and_then(|cursor| cursor.updated_since),
                )
                .flatten(),
            sync_tokens: (!complete_snapshot)
                .then_some(
                    stored_cursor
                        .as_ref()
                        .map(|cursor| cursor.sync_tokens.clone())
                        .unwrap_or_default(),
                )
                .unwrap_or_default(),
        };
        let batch = match self.transport.fetch(&request) {
            Ok(batch) => batch,
            Err(error) => {
                let _ = store.record_failed_reconcile(ctx, &error.to_string());
                return Err(error);
            }
        };

        let result = (|| -> Result<ReconcileResult> {
            let mut events = Vec::new();
            let mut attendees = Vec::new();
            let mut tombstones = Vec::new();
            let mut raw_items = Vec::new();
            for event in batch.events {
                let source_id = stable_source_id(&event)?;
                raw_items.push(RawItemDraft {
                    source_id: source_id.clone(),
                    payload: serde_json::to_value(&event)
                        .context("failed to preserve Calendar transport record")?,
                });
                if event.status.as_deref() == Some("cancelled") {
                    tombstones.push(source_id);
                    continue;
                }
                let occurred_at = event_start(&event.start).with_context(|| {
                    format!("Calendar event {} has no valid start time", event.id)
                })?;
                if occurred_at < scope.occurred_from || occurred_at > scope.occurred_to {
                    // A changed event can move outside the rolling collection.
                    // Exclude the old in-boundary materialization without purging
                    // its authoritative row or raw cache.
                    tombstones.push(source_id);
                    continue;
                }
                let (evidence, event_attendees) =
                    materialize_event(&event, source_id, occurred_at)?;
                events.push(evidence);
                attendees.extend(event_attendees);
            }

            store.apply_calendar_event_delta(
                ctx,
                CalendarEventDelta {
                    events,
                    attendees,
                    tombstone_source_ids: tombstones,
                    raw_items,
                    scope: scope_range,
                    complete_snapshot: complete_snapshot || batch.complete_snapshot,
                    materialization_fingerprint,
                    next_cursor: Some(batch.next_cursor),
                },
                expected_workspace_revision,
            )
        })();
        if let Err(error) = &result {
            if error.downcast_ref::<WorkspaceMutationError>().is_none() {
                let _ = store.record_failed_reconcile(ctx, &format!("{error:#}"));
            }
        }
        result
    }

    fn health(&self, ctx: &ConnectorCtx) -> Result<HealthReport> {
        validate_ctx(ctx)?;
        match self.transport.auth_state(&ctx.account)? {
            CalendarAuthState::Ready => {
                IntegrationsStore::open(&ctx.vault_root)?.health_report(ctx)
            }
            CalendarAuthState::NeedsAuth(detail) => Ok(HealthReport {
                status: HealthStatus::NeedsAuth,
                reason: Some("credentials_unavailable".to_string()),
                last_successful_sync: IntegrationsStore::open(&ctx.vault_root)?
                    .health_report(ctx)?
                    .last_successful_sync,
                cursor_age_secs: None,
                detail: Some(detail),
            }),
        }
    }
}

fn validate_ctx(ctx: &ConnectorCtx) -> Result<()> {
    if ctx.connector_id != GOOGLE_CALENDAR_CONNECTOR_ID {
        bail!("Google Calendar connector requires connector_id={GOOGLE_CALENDAR_CONNECTOR_ID}");
    }
    if ctx.account.trim().is_empty() {
        bail!("Google Calendar connector requires an explicit account")
    }
    Ok(())
}

fn materialize_event(
    event: &CalendarEvent,
    source_id: String,
    occurred_at: DateTime<Utc>,
) -> Result<(CalendarEventEvidence, Vec<CalendarEventAttendee>)> {
    let title = event
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Untitled calendar event")
        .to_string();
    let href = event
        .html_link
        .as_deref()
        .filter(|href| href.starts_with("https://"))
        .map(str::to_owned)
        .map(Ok)
        .unwrap_or_else(|| derived_google_calendar_url(event))?;
    let attendees = event_people(event)
        .into_iter()
        .enumerate()
        .map(
            |(position, (attendee, attendee_key, organizer))| CalendarEventAttendee {
                source_id: source_id.clone(),
                attendee_key,
                position: u32::try_from(position).expect("Calendar attendee count fits u32"),
                display_name: attendee_name(attendee),
                email: attendee.email.as_deref().map(normalize_email),
                response_status: attendee.response_status.clone(),
                is_self: attendee.is_self,
                organizer,
            },
        )
        .collect::<Vec<_>>();
    let occurred_to = event.end.as_ref().and_then(event_start);
    let body_text = render_event_text(event, &title);
    Ok((
        CalendarEventEvidence {
            source_id,
            calendar_id: event_calendar_id(event)?.to_string(),
            occurred_from: occurred_at,
            occurred_to,
            title,
            body_text,
            href: Some(href),
        },
        attendees,
    ))
}

fn render_event_text(event: &CalendarEvent, title: &str) -> String {
    let mut body = format!("Title: {title}\n");
    if let Some(start) = event_time_text(&event.start) {
        body.push_str(&format!("Starts: {start}\n"));
    }
    if let Some(end) = event.end.as_ref().and_then(event_time_text) {
        body.push_str(&format!("Ends: {end}\n"));
    }
    if let Some(location) = event.location.as_deref().filter(|value| !value.is_empty()) {
        body.push_str(&format!("Location: {location}\n"));
    }
    if let Some(description) = event
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body.push_str("\nDescription:\n");
        body.push_str(description);
        body.push('\n');
    }
    body
}

fn event_people(event: &CalendarEvent) -> Vec<(&CalendarAttendee, String, bool)> {
    let mut out = Vec::new();
    let mut seen_emails = BTreeSet::new();
    let organizer_email = event
        .organizer
        .as_ref()
        .and_then(|attendee| attendee.email.as_deref())
        .map(normalize_email);
    for (source_position, attendee) in event
        .attendees
        .iter()
        .chain(event.organizer.iter())
        .enumerate()
        .filter(|(_, attendee)| !attendee.resource)
    {
        let (key, organizer) = if let Some(email) = attendee.email.as_deref() {
            let email = normalize_email(email);
            if !seen_emails.insert(email.clone()) {
                continue;
            }
            (
                format!("email:{email}"),
                organizer_email.as_deref() == Some(email.as_str()),
            )
        } else {
            (
                format!("position:{source_position}"),
                source_position >= event.attendees.len(),
            )
        };
        out.push((attendee, key, organizer));
    }
    out
}

fn attendee_name(attendee: &CalendarAttendee) -> String {
    attendee
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| attendee.email.as_deref().map(email_display_name))
        .unwrap_or_else(|| "Unknown attendee".into())
}

fn email_display_name(email: &str) -> String {
    email
        .split('@')
        .next()
        .unwrap_or(email)
        .split(['.', '_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn event_calendar_id(event: &CalendarEvent) -> Result<&str> {
    event
        .calendar_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("Calendar event {} has no calendar identity", event.id))
}

fn stable_source_id(event: &CalendarEvent) -> Result<String> {
    Ok(format!("{}:{}", event_calendar_id(event)?, event.id))
}

fn derived_google_calendar_url(event: &CalendarEvent) -> Result<String> {
    let eid = URL_SAFE_NO_PAD.encode(format!("{} {}", event.id, event_calendar_id(event)?));
    Ok(format!(
        "https://calendar.google.com/calendar/event?eid={eid}"
    ))
}

fn event_start(value: &Value) -> Option<DateTime<Utc>> {
    event_time_text(value).and_then(|value| {
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|time| time.with_timezone(&Utc))
            .or_else(|| {
                chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                    .ok()?
                    .and_hms_opt(0, 0, 0)
                    .map(|time| time.and_utc())
            })
    })
}

fn event_time_text(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| {
        value
            .get("dateTime")
            .or_else(|| value.get("date"))
            .and_then(Value::as_str)
    })
}

struct CalendarStoredCursor {
    updated_since: Option<DateTime<Utc>>,
    sync_tokens: BTreeMap<String, String>,
}

fn parse_cursor(value: &Value) -> Result<CalendarStoredCursor> {
    let raw = value
        .get("updated_since")
        .and_then(Value::as_str)
        .context("Google Calendar cursor lacks updated_since")?;
    let updated_since = DateTime::parse_from_rfc3339(raw)
        .map(|time| time.with_timezone(&Utc))
        .context("Google Calendar cursor has invalid updated_since")?;
    let sync_tokens = value
        .get("sync_tokens")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|tokens| tokens.iter())
        .filter_map(|(calendar, token)| {
            token
                .as_str()
                .map(|token| (calendar.clone(), token.to_string()))
        })
        .collect();
    Ok(CalendarStoredCursor {
        updated_since: Some(updated_since),
        sync_tokens,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn google_event_envelopes_and_times_parse() {
        let payload = json!({"events": [{
            "id": "event-1",
            "CalendarID": "primary",
            "start": {"dateTime": "2026-08-24T10:00:00Z"},
            "attendees": []
        }]});
        let events = parse_calendar_events_from_value(payload).unwrap();
        assert_eq!(events[0].calendar_id.as_deref(), Some("primary"));
        assert_eq!(
            event_start(&events[0].start).unwrap(),
            "2026-08-24T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn fallback_provenance_is_an_openable_google_url() {
        let event = CalendarEvent {
            id: "event id".into(),
            calendar_id: Some("team@example.com".into()),
            status: None,
            summary: None,
            description: None,
            location: None,
            start: json!({"date": "2026-08-24"}),
            end: None,
            updated: None,
            html_link: None,
            attendees: vec![],
            organizer: None,
            extra: BTreeMap::new(),
        };
        let href = derived_google_calendar_url(&event).unwrap();
        assert!(href.starts_with("https://calendar.google.com/calendar/event?eid="));
        assert!(!href.contains(' '));
    }
}
