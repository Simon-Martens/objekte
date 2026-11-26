//! Query access to text candidates in a pinned collection base.

use roaring::RoaringBitmap;

use crate::index::storage::segment::SegmentReader;
use crate::index::storage::text::SegmentTextAccess;
use crate::query::binding::fts::BoundFts;
use crate::query_execution::domain::domain_id;
use crate::query_predicate::{PreparedPredicates, TermId};
use crate::repository::state::CollectionState;

use super::contract::{
    AccessExecution, AccessRequest, IndexAccessProvider, InputEstimate, OfferList,
};
use super::membership::{DomainId, SlotInput, SlotSet};
use super::registry::ProviderCatalog;

#[cfg(test)]
pub(crate) use crate::index::memory::access::text::test_text_input;

fn with_provider<T>(
    collection: &CollectionState,
    instance: u64,
    read: impl FnOnce(&dyn IndexAccessProvider, DomainId) -> Result<Option<T>, String>,
) -> Result<Option<T>, String> {
    let Some(base) = &collection.base_records else {
        return Ok(None);
    };
    let domain = domain_id(
        instance,
        collection.collection_id().0,
        collection.base_generation(),
        None,
    );
    let mut read = Some(read);
    let mut result = None;
    base.with_text_access(
        SegmentTextAccess {
            policy: collection.text_access_policy(),
            domain,
            instance,
            collection: collection.collection_id(),
        },
        &mut |provider| {
            let read = read.take().ok_or_else(|| {
                crate::index::storage::transfer::StorageError::invalid(
                    "open base text provider",
                    "provider scope was entered twice",
                )
            })?;
            result = Some(match provider {
                Some(provider) => read(provider, domain),
                None => Ok(None),
            });
            Ok(())
        },
    )?;
    result.ok_or_else(|| "base text access omitted provider selection".to_owned())?
}

fn estimate(collection: &CollectionState, domain: DomainId, input_len: u64) -> InputEstimate {
    InputEstimate {
        domain,
        domain_len: collection.base_slot_count() as u64,
        input_len,
        mean_record_bytes: collection.stats.mean_record_bytes(),
        mean_records_per_file: collection.stats.mean_records_per_file(),
        file_count: collection.stats.file_count as u64,
    }
}

pub(crate) fn base_text_estimate(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Result<Option<(u64, f64)>, String> {
    #[cfg(test)]
    if collection.base_records.is_none() {
        return Ok(
            crate::index::memory::access::text::native_estimate_fts_candidates(collection, fts),
        );
    }
    with_provider(
        collection,
        collection.repository_instance,
        |provider, domain| {
            let term = TermId::new(0);
            let input = collection.base_slot_count() as u64;
            let mut offers = OfferList::new();
            provider
                .propose(
                    term,
                    AccessRequest::Text(fts),
                    estimate(collection, domain, input),
                    &mut offers,
                )
                .map_err(|error| error.to_string())?;
            let chosen = crate::query_plan::selection::choose_offer(term, &offers, input)
                .map_err(|error| error.to_string())?;
            Ok(chosen.map(|offer| (offer.estimated_output(), offer.estimated_work(input))))
        },
    )
}

pub(crate) fn prepared_text_estimate(
    predicates: &PreparedPredicates,
    term: TermId,
    collection: &CollectionState,
    instance: u64,
    input_len: u64,
) -> Result<Option<(u64, f64)>, String> {
    #[cfg(test)]
    if collection.base_records.is_none() {
        return crate::index::memory::access::text::prepared_text_estimate(
            predicates, term, collection, instance, input_len,
        );
    }
    with_provider(collection, instance, |provider, domain| {
        let catalog = ProviderCatalog::new([provider]);
        let offers = catalog
            .propose_prepared(predicates, term, estimate(collection, domain, input_len))
            .map_err(|error| error.to_string())?;
        let chosen = crate::query_plan::selection::choose_offer(term, &offers, input_len)
            .map_err(|error| error.to_string())?;
        Ok(chosen.map(|offer| (offer.estimated_output(), offer.estimated_work(input_len))))
    })
}

pub(crate) fn prepared_text_candidates(
    predicates: &PreparedPredicates,
    term: TermId,
    collection: &CollectionState,
    instance: u64,
    input: &RoaringBitmap,
) -> Result<Option<RoaringBitmap>, String> {
    #[cfg(test)]
    if collection.base_records.is_none() {
        return crate::index::memory::access::text::prepared_text_candidates(
            predicates, term, collection, instance, input,
        );
    }
    with_provider(collection, instance, |provider, domain| {
        let catalog = ProviderCatalog::new([provider]);
        let offers = catalog
            .propose_prepared(predicates, term, estimate(collection, domain, input.len()))
            .map_err(|error| error.to_string())?;
        let Some(chosen) = crate::query_plan::selection::choose_offer(term, &offers, input.len())
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        let applied = catalog
            .apply(
                term,
                predicates
                    .request(term)
                    .map_err(|error| error.to_string())?,
                chosen.id(),
                SlotInput::unrestricted(domain, input),
                &AccessExecution { context: None },
            )
            .map_err(|error| error.to_string())?
            .possible;
        Ok((!matches!(applied.slots(), SlotSet::Unrestricted))
            .then(|| applied.into_owned_members()))
    })
}
