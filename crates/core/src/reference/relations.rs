//! Independent relation-aware evaluation over pinned visible records.

use std::collections::HashMap;
use std::sync::Arc;

use crate::index::segment::PublishedSegment;
use crate::inventory::ActiveRecords;
use crate::lifecycle::IndexRevision;
use crate::lifecycle::catalog::FileCatalog;
use crate::query::Query;
use crate::query::binding::filter::BoundExpr;
use crate::query::binding::fts::BoundFts;
use crate::repository::{Repository, SearchResult};

pub(crate) struct ReferenceCollectionQuery {
    pub(crate) collection_index: usize,
    pub(crate) filter: Option<BoundExpr>,
    pub(crate) fts: Option<BoundFts>,
}

pub(crate) fn bind(
    repo: &Repository,
    query: &Query,
) -> Result<Vec<ReferenceCollectionQuery>, String> {
    let mut parameter_types = HashMap::new();
    let mut collections = Vec::new();
    for (collection_index, collection) in repo.collections.iter().enumerate() {
        if !query.collections.is_empty() && !query.collections.contains(&collection.definition.name)
        {
            continue;
        }
        let filter = query
            .filter
            .as_ref()
            .map(|filter| {
                crate::query::binding::relation::bind_filter_with_relations(
                    repo,
                    collection_index,
                    filter,
                    &mut parameter_types,
                )
            })
            .transpose()?;
        let fts = query
            .fts
            .as_ref()
            .map(|fts| {
                crate::query::binding::relation::bind_fts_with_relations(
                    repo,
                    collection_index,
                    fts,
                )
            })
            .transpose()?;
        collections.push(ReferenceCollectionQuery {
            collection_index,
            filter,
            fts,
        });
    }
    if collections.is_empty() {
        return Err("unknown or unloaded collection scope".into());
    }
    Ok(collections)
}

pub(crate) fn search_segmented(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
) -> Result<SearchResult, String> {
    let mut collections = bind(repo, query)?;
    crate::query_execution::runner::search_segmented_reference_records(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        &mut collections,
    )
}
