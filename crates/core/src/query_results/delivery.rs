//! Bounded result delivery and replacements.

use std::collections::BinaryHeap;

use roaring::RoaringBitmap;

use crate::measurement::WorkCounters;
use crate::query::Query;
use crate::query_results::materialize::materialize_winner_page;
use crate::query_results::ranking::{Winner, rank_key_bytes};
use crate::repository::Repository;
use crate::streaming::{Delivery, ExecutionContext, StreamMode};
use crate::workload::ReturnMode;

fn winner_bytes(winner: &Winner) -> usize {
    std::mem::size_of::<Winner>()
        + rank_key_bytes(&winner.key) as usize
        + crate::streaming::hit_bytes(&winner.hit)
}

/// Publish a bounded provisional top-k page after a completed source batch.
pub(crate) fn emit_provisional_page(
    repo: &Repository,
    query: &Query,
    winners: &mut BinaryHeap<Winner>,
    counters: &mut WorkCounters,
    files_read: &mut Option<RoaringBitmap>,
    delivery: &mut Delivery<'_>,
    extra_retained: usize,
    execution: Option<&ExecutionContext>,
) -> Result<(), String> {
    delivery.check()?;
    let retained = winners.iter().map(winner_bytes).sum::<usize>();
    delivery.check_buffer(retained.saturating_add(extra_retained))?;
    if query.options.limit == 0 {
        return delivery.progress_event(counters);
    }
    if delivery.options().mode != StreamMode::PageReplacements {
        return Ok(());
    }
    let mut ranked = std::mem::take(winners).into_vec();
    ranked.sort_by(|a, b| a.key.cmp(&b.key));
    let start = query.options.offset.min(ranked.len());
    let end = start.saturating_add(query.options.limit).min(ranked.len());
    if start == end {
        *winners = ranked.into_iter().collect();
        return Ok(());
    }
    if !delivery.page_changed_identities(ranked[start..end].iter().map(|winner| {
        (
            winner.hit.collection.as_str(),
            winner.hit.record_id.as_str(),
        )
    })) {
        *winners = ranked.into_iter().collect();
        return Ok(());
    }
    let projected_copy = ranked[start..end].iter().fold(0usize, |total, winner| {
        let missing_body =
            if winner.hit.payload.is_none() && query.options.return_mode != ReturnMode::Locator {
                repo.collections[winner.collection_index].generation.records[winner.slot as usize]
                    .range
                    .len() as usize
            } else {
                0
            };
        total.saturating_add(winner_bytes(winner).saturating_add(missing_body))
    });
    let scratch = ranked
        .len()
        .saturating_add(end - start)
        .saturating_mul(std::mem::size_of::<Winner>());
    delivery.check_buffer(
        retained
            .saturating_add(extra_retained)
            .saturating_add(projected_copy)
            .saturating_add(scratch),
    )?;
    let mut page = ranked.drain(start..end).collect::<Vec<Winner>>();
    materialize_winner_page(
        repo,
        query,
        &mut page,
        counters,
        files_read,
        None,
        Some(delivery.token()),
        Some(delivery.max_buffered_bytes()),
        execution,
    )?;
    let mut owners = Vec::with_capacity(page.len());
    let mut hits = Vec::with_capacity(page.len());
    for winner in page.drain(..) {
        owners.push((
            winner.key,
            winner.collection_index,
            winner.slot,
            winner.retained_bytes,
        ));
        hits.push(winner.hit);
    }
    let delivered = hits.iter().map(crate::streaming::hit_bytes).sum::<usize>();
    delivery.replacement(
        &hits,
        counters,
        retained
            .saturating_add(extra_retained)
            .saturating_add(delivered),
    )?;
    ranked.extend(owners.into_iter().zip(hits).map(
        |((key, collection_index, slot, retained_bytes), hit)| Winner {
            key,
            collection_index,
            slot,
            hit,
            retained_bytes,
        },
    ));
    *winners = ranked.into_iter().collect();
    Ok(())
}
