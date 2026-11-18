//! Pinned local slot-domain identity.

use crate::query_access::membership::DomainId;

pub(crate) fn domain_id(collection: u32, revision: u64, segment: u32) -> DomainId {
    let tag = (u64::from(collection) << 48)
        ^ (revision.rotate_left(17) & 0x0000_ffff_ffff_0000)
        ^ u64::from(segment);
    DomainId::new(tag)
}
