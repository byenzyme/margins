//! Portable application workflows for Margins.
#![forbid(unsafe_code)]

pub mod agents;
pub mod alignment;
pub mod artifacts;
pub mod granola_import;
pub mod note_artifacts;
pub mod processing;
pub mod project;
pub mod publish;
pub mod session_index;
pub mod transcript_view;

pub mod resources {
    pub const ENZYME_WORKSPACE_SETUP_SKILL: &str =
        include_str!("../resources/skills/enzyme-workspace-setup/SKILL.md");
    pub const MARGINS_WORKSPACE_SETUP_SKILL: &str =
        include_str!("../resources/skills/margins-workspace-setup/SKILL.md");
    pub const MARGINS_AGENT_INSTRUCTIONS: &str = include_str!("../resources/agents/margins.md");
}
