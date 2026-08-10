//! Portable media implementation shared by Margins composition crates.
//!
//! Native capture ownership, callback transport, durability policy, and desktop
//! integration intentionally live elsewhere. Optional model adapters accept
//! caller-supplied PCM and never open devices.

#![deny(unsafe_code)]

pub mod audio;
pub mod diarization;
pub mod info;
pub mod providers;
pub mod timeline;
pub mod transcript;
