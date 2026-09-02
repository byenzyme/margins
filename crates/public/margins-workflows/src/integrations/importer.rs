//! In-memory, one-shot platform migration contract.

use anyhow::Result;

/// An importer surveys source material in memory and writes native files only
/// after the caller supplies approved options. Unlike [`super::Connector`], it
/// has no durable source state, cursor, receipt, freshness, tombstone, or
/// recurring health semantics.
pub trait Importer {
    type Survey;
    type Options;
    type Output;

    fn survey(&self) -> Result<Self::Survey>;

    fn import(self, options: &Self::Options) -> Result<Self::Output>;
}
