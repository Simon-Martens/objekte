//! Pinned local slot-domain identity.

use crate::query_access::membership::DomainId;

pub(crate) fn domain_id(
    repository: u64,
    collection: u32,
    revision: u64,
    segment: Option<u64>,
) -> DomainId {
    DomainId::from_parts(repository, collection, revision, segment)
}
