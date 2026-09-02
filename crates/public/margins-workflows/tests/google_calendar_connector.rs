//! Fixture-driven Google Calendar connector acceptance tests (no live account).

use anyhow::Result;
use chrono::{DateTime, TimeZone, Utc};
use margins_workflows::integrations::{
    CalendarAttendee, CalendarAuthState, CalendarEvent, CalendarEventEvidence, CalendarFetchBatch,
    CalendarFetchRequest, CalendarTransport, Connector, ConnectorCtx, GoogleCalendarConnector,
    HealthStatus, IntegrationsStore, GOOGLE_CALENDAR_CONNECTOR_ID,
};
use margins_workflows::workspace::CalendarCollectionSelector;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct FixtureCalendarTransport {
    events: Arc<Mutex<Vec<CalendarEvent>>>,
    requests: Arc<Mutex<Vec<CalendarFetchRequest>>>,
    complete_snapshot: Arc<Mutex<bool>>,
    next_cursor: DateTime<Utc>,
}

impl FixtureCalendarTransport {
    fn from_fixture() -> Self {
        let payload: Value =
            serde_json::from_str(include_str!("fixtures/google_calendar_events.json")).unwrap();
        let events = serde_json::from_value(payload["events"].clone()).unwrap();
        Self {
            events: Arc::new(Mutex::new(events)),
            requests: Arc::new(Mutex::new(Vec::new())),
            complete_snapshot: Arc::new(Mutex::new(false)),
            next_cursor: Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0).unwrap(),
        }
    }
}

impl CalendarTransport for FixtureCalendarTransport {
    fn fetch(&self, request: &CalendarFetchRequest) -> Result<CalendarFetchBatch> {
        self.requests.lock().unwrap().push(request.clone());
        let calendars = request
            .calendars
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        let events = self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| {
                calendars.is_empty()
                    || event
                        .calendar_id
                        .as_ref()
                        .map(|calendar| calendars.contains(calendar))
                        .unwrap_or(false)
            })
            .cloned()
            .collect();
        Ok(CalendarFetchBatch {
            events,
            next_cursor: json!({
                "updated_since": self.next_cursor.to_rfc3339()
            }),
            complete_snapshot: *self.complete_snapshot.lock().unwrap(),
        })
    }

    fn auth_state(&self, _account: &str) -> Result<CalendarAuthState> {
        Ok(CalendarAuthState::Ready)
    }
}

fn selector() -> CalendarCollectionSelector {
    CalendarCollectionSelector {
        lookback_days: 30,
        lookahead_days: 30,
    }
}

fn connector(
    transport: FixtureCalendarTransport,
) -> GoogleCalendarConnector<FixtureCalendarTransport> {
    GoogleCalendarConnector::new_at(
        transport,
        selector(),
        Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0).unwrap(),
    )
}

fn ctx(vault: &std::path::Path) -> ConnectorCtx {
    ConnectorCtx {
        vault_root: vault.to_path_buf(),
        connector_id: GOOGLE_CALENDAR_CONNECTOR_ID.into(),
        account: "morgan@enzyme.dev".into(),
        command_path: None,
    }
}

#[test]
fn reconcile_is_fixture_driven_and_idempotent() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let transport = FixtureCalendarTransport::from_fixture();
    let connector = connector(transport.clone());
    let ctx = ctx(temp.path());

    let first = connector.reconcile(&ctx, None)?;
    assert_eq!(first.records_written, 2);
    assert_eq!(first.tombstones, 0, "unknown cancellations are harmless");
    let store = IntegrationsStore::open(temp.path())?;
    assert_eq!(store.calendar_event_evidence(&ctx)?.len(), 2);
    assert!(store.calendar_event_attendees(&ctx)?.len() >= 4);

    let second = connector.reconcile(&ctx, None)?;
    assert_eq!(second.records_written, 0);
    assert_eq!(second.records_updated, 0);
    assert_eq!(second.records_unchanged, 2);

    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "one provider fetch per reconcile");
    assert!(
        requests[0].updated_since.is_none(),
        "first reconcile is a snapshot"
    );
    assert_eq!(
        requests[1].updated_since,
        Some(Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0).unwrap()),
        "second reconcile uses the persisted changed-event cursor"
    );
    Ok(())
}

#[test]
fn missing_calendar_identity_fails_refresh_without_colliding_across_calendars() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let transport = FixtureCalendarTransport::from_fixture();
    {
        let mut events = transport.events.lock().unwrap();
        events.truncate(2);
        for event in events.iter_mut() {
            event.id = "shared-event-id".into();
            event.calendar_id = None;
        }
    }
    let connector = connector(transport);
    let ctx = ctx(temp.path());
    let error = connector.reconcile(&ctx, None).unwrap_err();
    assert!(error
        .to_string()
        .contains("Calendar event shared-event-id has no calendar identity"));
    let store = IntegrationsStore::open(temp.path())?;
    assert!(store.calendar_event_evidence(&ctx)?.is_empty());
    assert_eq!(connector.health(&ctx)?.status, HealthStatus::Error);
    Ok(())
}

#[test]
fn name_only_attendees_keep_source_position_identity() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let transport = FixtureCalendarTransport::from_fixture();
    {
        let mut events = transport.events.lock().unwrap();
        events.truncate(1);
        events[0].attendees = vec![
            CalendarAttendee {
                display_name: Some("Alex Smith".into()),
                email: None,
                resource: false,
                is_self: false,
                response_status: Some("accepted".into()),
            },
            CalendarAttendee {
                display_name: Some("Alex Smith".into()),
                email: None,
                resource: false,
                is_self: false,
                response_status: Some("tentative".into()),
            },
        ];
    }
    let connector = connector(transport);
    let ctx = ctx(temp.path());
    connector.reconcile(&ctx, None)?;
    let attendees = IntegrationsStore::open(temp.path())?.calendar_event_attendees(&ctx)?;
    let alexes = attendees
        .iter()
        .filter(|attendee| attendee.display_name == "Alex Smith")
        .collect::<Vec<_>>();
    assert_eq!(alexes.len(), 2);
    assert_eq!(alexes[0].attendee_key, "position:0");
    assert_eq!(alexes[1].attendee_key, "position:1");
    Ok(())
}

#[test]
fn materialized_events_preserve_attendees_and_openable_provenance_without_projection() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    let connector = connector(FixtureCalendarTransport::from_fixture());
    let ctx = ctx(temp.path());
    connector.reconcile(&ctx, None)?;
    let store = IntegrationsStore::open(temp.path())?;

    let events = store.calendar_event_evidence(&ctx)?;
    let kickoff = events
        .iter()
        .find(|event| event.source_id == "primary:evt-client-kickoff")
        .expect("kickoff evidence");
    assert_eq!(
        kickoff.href.as_deref(),
        Some("https://calendar.google.com/calendar/event?eid=fixture-acme-kickoff"),
        "the authoritative Google event link remains the receipt"
    );
    assert!(!kickoff.body_text.contains("##"));
    assert!(!store.calendar_event_attendees(&ctx)?.is_empty());
    assert!(!temp.path().join("Margins").exists());

    let review = events
        .iter()
        .find(|event| event.source_id == "team@enzyme.dev:evt-design-review")
        .expect("review evidence");
    assert!(review
        .href
        .as_deref()
        .unwrap()
        .starts_with("https://calendar.google.com/calendar/event?eid="));
    assert!(!review.href.as_deref().unwrap().contains(' '));

    let health = connector.health(&ctx)?;
    assert_eq!(health.status, HealthStatus::Fresh);
    Ok(())
}

#[test]
fn changed_event_cancellation_tombstones_authoritative_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let transport = FixtureCalendarTransport::from_fixture();
    let connector = connector(transport.clone());
    let ctx = ctx(temp.path());
    connector.reconcile(&ctx, None)?;

    {
        let mut events = transport.events.lock().unwrap();
        let event = events
            .iter_mut()
            .find(|event| event.id == "evt-client-kickoff")
            .unwrap();
        event.status = Some("cancelled".into());
    }
    let changed = connector.reconcile(&ctx, None)?;
    assert_eq!(changed.tombstones, 1);
    let store = IntegrationsStore::open(temp.path())?;
    assert!(!store
        .calendar_event_evidence(&ctx)?
        .iter()
        .any(|event| event.source_id == "primary:evt-client-kickoff"));
    Ok(())
}

#[test]
fn forced_complete_snapshot_tombstones_removed_calendar_collection_members() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let transport = FixtureCalendarTransport::from_fixture();
    let connector = connector(transport.clone());
    let ctx = ctx(temp.path());
    connector.reconcile(&ctx, None)?;

    {
        let mut events = transport.events.lock().unwrap();
        events.retain(|event| event.id != "evt-client-kickoff");
        *transport.complete_snapshot.lock().unwrap() = true;
    }
    let changed = connector.reconcile(&ctx, None)?;
    assert_eq!(changed.tombstones, 1);
    let store = IntegrationsStore::open(temp.path())?;
    assert!(!store
        .calendar_event_evidence(&ctx)?
        .iter()
        .any(|event| event.source_id == "primary:evt-client-kickoff"));
    Ok(())
}

#[test]
fn changed_event_replaces_attendee_associations() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let transport = FixtureCalendarTransport::from_fixture();
    let connector = connector(transport.clone());
    let ctx = ctx(temp.path());
    connector.reconcile(&ctx, None)?;

    {
        let mut events = transport.events.lock().unwrap();
        let event = events
            .iter_mut()
            .find(|event| event.id == "evt-client-kickoff")
            .unwrap();
        event.attendees = vec![CalendarAttendee {
            display_name: Some("Grace Hopper".into()),
            email: Some("grace@acme.example".into()),
            resource: false,
            is_self: false,
            response_status: Some("accepted".into()),
        }];
    }

    let changed = connector.reconcile(&ctx, None)?;
    assert_eq!(changed.records_updated, 1);
    let attendees = IntegrationsStore::open(temp.path())?.calendar_event_attendees(&ctx)?;
    let kickoff = attendees
        .iter()
        .filter(|attendee| attendee.source_id == "primary:evt-client-kickoff")
        .collect::<Vec<_>>();
    assert!(kickoff
        .iter()
        .any(|attendee| attendee.email.as_deref() == Some("grace@acme.example")));
    assert!(!kickoff
        .iter()
        .any(|attendee| attendee.email.as_deref() == Some("kevin.chen@beta.example")));
    Ok(())
}

#[test]
fn complete_snapshot_tombstones_absent_events_and_raw_cache_is_independent() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let transport = FixtureCalendarTransport::from_fixture();
    let initial = connector(transport.clone());
    let ctx = ctx(temp.path());
    initial.reconcile(&ctx, None)?;

    transport
        .events
        .lock()
        .unwrap()
        .retain(|event| event.id != "evt-client-kickoff");
    let changed_selector = CalendarCollectionSelector {
        lookback_days: 29,
        lookahead_days: 30,
    };
    let refreshed = GoogleCalendarConnector::new_at(
        transport,
        changed_selector,
        Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0).unwrap(),
    );
    let result = refreshed.reconcile(&ctx, None)?;
    assert_eq!(result.tombstones, 1);
    let store = IntegrationsStore::open(temp.path())?;
    assert!(!store
        .calendar_event_evidence(&ctx)?
        .iter()
        .any(|event| event.source_id == "primary:evt-client-kickoff"));
    assert!(store
        .raw_items(&ctx)?
        .iter()
        .any(|raw| raw.source_id == "primary:evt-client-kickoff"));

    rusqlite::Connection::open(store.db_path())?.execute(
        "DELETE FROM raw_items WHERE connector_id = 'gcal' AND source_account = ?1",
        [&ctx.account],
    )?;
    assert_eq!(store.raw_items(&ctx)?.len(), 0);
    assert_eq!(store.calendar_event_evidence(&ctx)?.len(), 1);
    Ok(())
}

#[test]
fn complete_snapshot_tombstones_only_events_within_materialization_scope() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let store = IntegrationsStore::open(temp.path())?;
    let ctx = ctx(temp.path());
    let anchor = Utc.with_ymd_and_hms(2026, 8, 24, 12, 0, 0).unwrap();
    let in_scope = anchor - chrono::Duration::days(5);
    let out_of_scope = anchor - chrono::Duration::days(120);
    let initial_selector = CalendarCollectionSelector {
        lookback_days: 180,
        lookahead_days: 30,
    };
    let selector = CalendarCollectionSelector {
        lookback_days: 30,
        lookahead_days: 30,
    };
    let initial_scope = margins_workflows::integrations::GoogleCalendarScope::for_selector(
        &initial_selector,
        anchor,
    )
    .unwrap()
    .as_range();
    let scope =
        margins_workflows::integrations::GoogleCalendarScope::for_selector(&selector, anchor)
            .unwrap()
            .as_range();
    store.apply_calendar_event_delta(
        &ctx,
        margins_workflows::integrations::CalendarEventDelta {
            events: vec![
                CalendarEventEvidence {
                    source_id: "primary:in-scope".into(),
                    calendar_id: "primary".into(),
                    occurred_from: in_scope,
                    occurred_to: Some(in_scope + chrono::Duration::hours(1)),
                    title: "Inside scope".into(),
                    body_text: "Inside scope body".into(),
                    href: None,
                },
                CalendarEventEvidence {
                    source_id: "primary:out-of-scope".into(),
                    calendar_id: "primary".into(),
                    occurred_from: out_of_scope,
                    occurred_to: Some(out_of_scope + chrono::Duration::hours(1)),
                    title: "Outside scope".into(),
                    body_text: "Outside scope body".into(),
                    href: None,
                },
            ],
            attendees: Vec::new(),
            tombstone_source_ids: Vec::new(),
            raw_items: Vec::new(),
            scope: initial_scope,
            complete_snapshot: false,
            materialization_fingerprint: initial_selector.materialization_fingerprint().unwrap(),
            next_cursor: None,
        },
        None,
    )?;
    store.apply_calendar_event_delta(
        &ctx,
        margins_workflows::integrations::CalendarEventDelta {
            events: Vec::new(),
            attendees: Vec::new(),
            tombstone_source_ids: Vec::new(),
            raw_items: Vec::new(),
            scope,
            complete_snapshot: true,
            materialization_fingerprint: selector.materialization_fingerprint().unwrap(),
            next_cursor: None,
        },
        None,
    )?;
    let connection = rusqlite::Connection::open(store.db_path())?;
    let in_scope_tombstone: Option<String> = connection.query_row(
        "SELECT tombstoned_at FROM calendar_event_evidence WHERE source_id = 'primary:in-scope'",
        [],
        |row| row.get(0),
    )?;
    let out_of_scope_tombstone: Option<String> = connection.query_row(
        "SELECT tombstoned_at FROM calendar_event_evidence WHERE source_id = 'primary:out-of-scope'",
        [],
        |row| row.get(0),
    )?;
    assert!(
        in_scope_tombstone.is_some(),
        "complete snapshots tombstone absent in-scope rows"
    );
    assert!(
        out_of_scope_tombstone.is_none(),
        "authoritative rows outside delta.scope survive complete snapshots"
    );
    Ok(())
}

#[test]
fn failed_refresh_preserves_snapshot_and_marks_health_error() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let transport = FixtureCalendarTransport::from_fixture();
    let connector = connector(transport.clone());
    let ctx = ctx(temp.path());
    connector.reconcile(&ctx, None)?;
    let store = IntegrationsStore::open(temp.path())?;
    let before = store.calendar_event_evidence(&ctx)?;

    transport.events.lock().unwrap()[0].start = serde_json::json!({});
    let error = connector.reconcile(&ctx, None).unwrap_err();
    assert!(error.to_string().contains("has no valid start time"));
    assert_eq!(store.calendar_event_evidence(&ctx)?, before);
    let health = connector.health(&ctx)?;
    assert_eq!(health.status, HealthStatus::Error);
    assert!(health
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("has no valid start time")));
    Ok(())
}
