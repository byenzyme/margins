//! Public integrations module surface.

mod connector;
mod email;
mod google_calendar;
mod google_meet;
mod google_native;
mod granola_native;
mod importer;
mod store;
mod types;

pub use connector::Connector;
pub use email::{
    automatic_correspondent_entities, automatic_correspondent_entity_selection,
    normalize_email_identity, parse_gmail_thread_from_value, CorrespondentEntity,
    CorrespondentEntitySelection, FetchedThread, GmailSearchPage, GmailThreadTransport,
    GoogleEmailConnector, EMAIL_CONNECTOR_ID,
};
pub use google_calendar::{
    CalendarAttendee, CalendarAuthState, CalendarEvent, CalendarFetchBatch, CalendarFetchRequest,
    CalendarTransport, GoogleCalendarConnector, GoogleCalendarScope, GOOGLE_CALENDAR_CONNECTOR_ID,
};
pub use google_meet::{
    parse_meet_v2_bundle, CalendarAttendeeEvidence, DriveTranscriptDocument, GoogleMeetConnector,
    GoogleMeetSnapshot, GoogleMeetTransport, MeetConferenceRecord, MeetParticipantEvidence,
    MeetTranscriptMetadata, GOOGLE_MEET_CONNECTOR_ID, GOOGLE_MEET_MATERIALIZATION_FINGERPRINT,
};
pub use google_native::{
    connect_google_account, redact_secret_text, validate_private_file_permissions,
    GoogleAccountStore, GoogleConnectionMetadata, GoogleConnectionReady,
    GoogleCredentialBackendKind, GoogleNativeError, GoogleOAuthMode, GoogleOAuthPresenter,
    GoogleTokenProvider, NativeGoogleClient, CALENDAR_READONLY_SCOPE, DOCS_READONLY_SCOPE,
    DRIVE_READONLY_SCOPE, GMAIL_READONLY_SCOPE, MEET_READONLY_SCOPE, REQUIRED_GOOGLE_SCOPES,
};
pub use granola_native::{
    connect_granola_account, fetch_granola_import_batch, granola_failure_info,
    list_granola_accounts, sync_granola_binding, GranolaAccountStore, GranolaConnectionMetadata,
    GranolaConnectionReady, GranolaCredentialBackendKind, GranolaFailureInfo,
    GranolaMcpImportBatch, GranolaNativeError, GranolaOAuthMode, GranolaOAuthPresenter,
    GRANOLA_CONNECTOR_ID, GRANOLA_MCP_URL, GRANOLA_PROTECTED_RESOURCE_METADATA_URL,
};
pub use importer::Importer;
pub use store::{apply_retention, preview_retention, IntegrationsStore};
pub use types::{
    CalendarEventAttendee, CalendarEventDelta, CalendarEventEvidence, ConnectorCtx,
    CurationObservation, EmailThreadSnapshotCounts, EvidenceFreshness, EvidenceHandle,
    ExternalDocumentDelta, ExternalDocumentEvidence, ExternalDocumentParticipant, FreshnessStatus,
    HealthReport, HealthStatus, InferredEntity, KindCounts, ParticipantThread, PersonIdentity,
    RankedParticipant, RawItemDraft, ReconcileResult, RetentionApplyReceipt, RetentionCounts,
    RetentionCutoffs, RetentionMutationError, RetentionPreview, RetentionScope, RetentionTarget,
    RunManifest, SurveyRange, ThreadEvidence, RETENTION_APPLY_SCHEMA, RETENTION_PREVIEW_SCHEMA,
};
