use roaring::RoaringBitmap;

mod inference;
mod postings;
mod routing;
mod sorting;
mod text;

pub(crate) use inference::project_inference;
pub(crate) use postings::{PostingProjection, project_posting, transfer_posting};
pub(crate) use routing::{PartitionRouting, SlotMap, project_bitmap};
pub(crate) use sorting::project_sorting;
pub(crate) use text::{BigramProjection, project_bigram, transfer_bigram};

use super::IndexedCollectionSegment;
use crate::index::memory::relation::RelationContribution;
use routing::scatter_bitmap_partitions;

impl IndexedCollectionSegment {
    pub(crate) fn project_relations_for_partitions(
        &self,
        routing: &PartitionRouting,
    ) -> Result<Vec<Vec<RelationContribution>>, String> {
        let counts = routing
            .selected
            .iter()
            .map(|slots| u32::try_from(slots.len()).map_err(|_| "partition slot overflow".into()))
            .collect::<Result<Vec<_>, String>>()?;
        let mut projected = counts
            .iter()
            .map(|_| Vec::with_capacity(self.relations.len()))
            .collect::<Vec<_>>();
        for relation in &self.relations {
            let parts = relation.project_partitions(&counts, |old| {
                routing
                    .locations
                    .get(old as usize)
                    .copied()
                    .ok_or("relation partition source outside routing".into())
            })?;
            for (destination, contribution) in projected.iter_mut().zip(parts) {
                destination.push(contribution);
            }
        }
        Ok(projected)
    }
}

pub(crate) struct DomainProjection {
    pub(crate) logical: RoaringBitmap,
    pub(crate) empty_array: RoaringBitmap,
}

impl IndexedCollectionSegment {
    pub(crate) fn project_domains_for_partitions(
        &self,
        routing: &PartitionRouting,
    ) -> Result<Vec<DomainProjection>, String> {
        let logical = scatter_bitmap_partitions(&self.logical_records, routing)?;
        let empty_array = scatter_bitmap_partitions(&self.empty_array_records, routing)?;
        if logical
            .iter()
            .zip(&routing.logical_counts)
            .any(|(domain, &count)| domain.len() != count as u64)
        {
            return Err("partition logical domain count mismatch".into());
        }
        Ok(logical
            .into_iter()
            .zip(empty_array)
            .map(|(logical, empty_array)| DomainProjection {
                logical,
                empty_array,
            })
            .collect())
    }
}
