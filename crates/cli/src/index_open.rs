//! Shared opening, checking, and saving of an explicitly selected index.

use std::path::Path;

use objekte_search_core::lifecycle::{Index, ReconciliationReport};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceRegistry;
use objekte_search_core::streaming::CancellationToken;
use objekte_search_core::SaveOptions;

/// Load one saved index with runtime definitions and source registrations.
pub fn load(
    directory: &Path,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    sources: SourceRegistry,
) -> Result<Index, String> {
    Index::load_from(directory, definitions, options, sources).map_err(|error| error.to_string())
}

/// Run one metadata reconciliation and wait for its bounded report.
pub fn check(index: &Index) -> Result<ReconciliationReport, String> {
    index
        .start_reconcile()
        .map_err(|error| error.to_string())?
        .wait()
        .map_err(|error| error.to_string())
}

/// Save one pinned publication, honouring the command cancellation token.
pub fn save(
    index: &Index,
    directory: &Path,
    token: &CancellationToken,
) -> Result<objekte_search_core::SaveReport, String> {
    index
        .save_to_with_control(
            directory,
            SaveOptions::default(),
            || token.is_cancelled(),
            |_| {},
        )
        .map_err(|error| error.to_string())
}
