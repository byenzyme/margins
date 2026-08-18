//! Portable application workflows for Margins.
#![forbid(unsafe_code)]

pub mod agents;
pub mod alignment;
pub mod archive;
pub mod artifacts;
pub mod granola_import;
pub mod note_artifacts;
pub mod processing;
pub mod project;
pub mod publish;
pub mod session_index;
pub mod transcript_view;

pub mod resources {
    pub const MARGINS_AGENT_INSTRUCTIONS: &str = include_str!("../resources/agents/margins.md");
    /// The single source of truth for the margins-native workspace-setup guide
    /// printed by `margins guide workspace-setup`.
    pub const MARGINS_WORKSPACE_SETUP_GUIDE: &str =
        include_str!("../resources/skills/margins-workspace-setup/SKILL.md");
}
