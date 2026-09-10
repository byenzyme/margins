//! Connector trait from the integrations connector contract.

use anyhow::Result;

use super::types::{ConnectorCtx, HealthReport, ReconcileResult};

/// Portable connector surface implemented by each integration source.
pub trait Connector {
    /// Reconcile the workspace-declared collection. Repeating the same coherent
    /// snapshot must be idempotent. When supplied, the workspace revision is
    /// checked again immediately before the authoritative ledger commit.
    fn reconcile(
        &self,
        ctx: &ConnectorCtx,
        expected_workspace_revision: Option<&str>,
    ) -> Result<ReconcileResult>;

    /// Cheap freshness/health check for status surfaces.
    fn health(&self, ctx: &ConnectorCtx) -> Result<HealthReport>;
}
