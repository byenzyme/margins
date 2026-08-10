//! Portable domain values and synchronous in-process ports for Margins.
//!
//! This crate owns no devices, platform handles, database connections, async
//! runtimes, or UI transports. Implementations live in composition crates and
//! communicate through owned values and typed errors defined here.

#![forbid(unsafe_code)]

pub mod audio;
pub mod capture;
pub mod event;
pub mod ids;
pub mod memo;
pub mod session;
pub mod transcript;

/// Versioned, transport-neutral meeting protocol DTOs.
///
/// These are re-exported instead of duplicated so a transport boundary has one
/// canonical wire representation.
pub mod wire {
    pub use margins_meeting_protocol::*;
}

pub use audio::*;
pub use capture::*;
pub use event::*;
pub use ids::*;
pub use memo::*;
pub use session::*;
pub use transcript::*;
