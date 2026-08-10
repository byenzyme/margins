//! Parsed memo values shared by command and workflow implementations.

use serde::{Deserialize, Serialize};

/// One parsed memo line anchored to the session timeline when available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}
