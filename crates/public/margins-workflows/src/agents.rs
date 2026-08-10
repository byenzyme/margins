//! Idempotent installation of public agent instructions.

use anyhow::{Context, Result};
use std::path::Path;

use crate::resources::MARGINS_AGENT_INSTRUCTIONS;

const BEGIN: &str = "<!-- BEGIN MARGINS AGENT INSTRUCTIONS -->";
const END: &str = "<!-- END MARGINS AGENT INSTRUCTIONS -->";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFileAction {
    Created,
    Updated,
    Unchanged,
}

impl AgentFileAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
        }
    }
}

pub fn write_agent_instruction_file(path: &Path) -> Result<AgentFileAction> {
    let existing = match std::fs::read_to_string(path) {
        Ok(content) => Some(content),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        }
    };
    let Some(existing) = existing else {
        std::fs::write(path, MARGINS_AGENT_INSTRUCTIONS)
            .with_context(|| format!("failed to write {}", path.display()))?;
        return Ok(AgentFileAction::Created);
    };
    let next = upsert_agent_instruction_block(&existing, MARGINS_AGENT_INSTRUCTIONS);
    if next == existing {
        return Ok(AgentFileAction::Unchanged);
    }
    std::fs::write(path, next).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(AgentFileAction::Updated)
}

pub fn upsert_agent_instruction_block(existing: &str, block: &str) -> String {
    if let Some(begin) = existing.find(BEGIN) {
        let search_from = begin + BEGIN.len();
        if let Some(relative_end) = existing[search_from..].find(END) {
            let end = search_from + relative_end + END.len();
            let mut next = String::with_capacity(existing.len() + block.len());
            next.push_str(&existing[..begin]);
            next.push_str(block.trim_end_matches('\n'));
            next.push_str(&existing[end..]);
            return next;
        }
    }
    let mut next = existing.to_string();
    if !next.ends_with('\n') {
        next.push('\n');
    }
    if !next.trim().is_empty() {
        next.push('\n');
    }
    next.push_str(block);
    next
}
