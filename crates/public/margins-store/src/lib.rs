//! Portable local persistence for Margins.
//!
//! The crate has two deliberately separate layers:
//!
//! - [`legacy`] preserves the original path-based SQLite API and on-disk schema.
//! - [`SqliteSessionRepository`] implements the richer `margins-core` port using
//!   additive sidecar tables for revision and lossless segment metadata.
//!
//! Native capture and desktop policy are intentionally absent.

#![forbid(unsafe_code)]

pub mod index;
pub mod legacy;
mod sqlite;

pub use index::{list_session_index, SessionIndexEntry, SessionIndexQuery};
pub use sqlite::SqliteSessionRepository;
