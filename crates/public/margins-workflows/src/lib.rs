//! Portable application workflows for Margins.
#![forbid(unsafe_code)]

pub mod agents;
pub mod alignment;
pub mod archive;
pub mod artifacts;
pub mod catalyst;
pub mod granola_import;
pub mod integrations;
pub mod local_recall;
pub mod machine_config;
pub mod note_artifacts;
pub mod processing;
pub mod project;
pub mod session_index;
pub mod source_kinds;
pub mod transcript_view;
/// The Workspace language crate, re-exported so every Margins crate uses the
/// one copy margins-workflows declares.
pub use enzyme_spec;
pub mod workspace;
pub mod workspace_preset;
pub mod workspace_program;
pub mod workspace_service;
pub mod remote_workspace;

pub mod resources {
    pub const MARGINS_AGENT_INSTRUCTIONS: &str = include_str!("../resources/agents/margins.md");
    /// Margins-owned commands, state, and permission policy for Workspace setup.
    pub const MARGINS_WORKSPACE_SETUP_ADAPTER: &str =
        include_str!("../resources/skills/margins-workspace-setup/SKILL.md");
    /// Product-neutral interpretation and insight-handoff guidance, pinned into
    /// this build from the Enzyme setup skill's shared reference.
    pub const KNOWLEDGE_PRACTICE_REVIEW: &str =
        include_str!("../resources/guidance/knowledge-practice-review.md");

    /// The complete guide printed by `margins guide workspace-setup`.
    pub fn margins_workspace_setup_guide() -> String {
        format!(
            "{}\n\n{}\n",
            MARGINS_WORKSPACE_SETUP_ADAPTER.trim_end(),
            KNOWLEDGE_PRACTICE_REVIEW.trim()
        )
    }
    /// The plain-language first-run router printed by `margins guide onboarding`.
    /// The canonical setup procedure remains the assembled Workspace setup guide.
    pub const MARGINS_GUIDED_ONBOARDING: &str =
        include_str!("../resources/skills/margins-guided-onboarding/SKILL.md");
}
