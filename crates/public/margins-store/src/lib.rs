//! Portable local persistence for Margins.
//!
//! The crate has two deliberately separate layers:
//!
//! - [`canonical`] owns the original on-disk schema and all production session
//!   mutations while data is converted additively.
//! - [`SqliteSessionRepository`] implements the richer `margins-core` port using
//!   additive sidecar tables for revision and lossless segment metadata.
//!
//! Native capture and desktop policy are intentionally absent.

#![forbid(unsafe_code)]

#[path = "legacy.rs"]
pub mod canonical;
mod authority;
mod meeting_runtime;
mod sqlite;

pub use authority::{
    AuthorityMemoReceipt, ImportReceipt, MemoRevisionConflict, MemoWrite,
    SqliteWorkspaceAuthorityStorage,
};
pub use meeting_runtime::{MeetingRuntimeStorageStats, SqliteMeetingRuntimeStorage};
pub use sqlite::SqliteSessionRepository;
