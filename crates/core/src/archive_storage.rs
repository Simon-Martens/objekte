//! Checked ownership for immutable in-memory rkyv archives.
//!
//! The aligned byte buffer is serialized and validated exactly once. After
//! construction, callers can only borrow an archived root for a lifetime tied
//! to this owner.

use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use rkyv::bytecheck::CheckBytes;
use rkyv::api::high::HighSerializer;
use rkyv::rancor::Error;
use rkyv::ser::allocator::ArenaHandle;
use rkyv::util::AlignedVec;
use rkyv::{Archive, Archived, Portable, Serialize, access};

use crate::storage_usage::StorageWorkCounters;

// Fixed experiment policy from the repair specification. The pinned aligned
// vector supports shrinking before validation, but production shrinking stays
// disabled unless matched release measurements justify the copy.
const SHRINK_OVERALLOCATED_ARCHIVES: bool = false;

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_FREEZE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn fail_next_freeze() {
    FAIL_NEXT_FREEZE.with(|fail| fail.set(true));
}

/// A serialized and checked archive whose aligned allocation is immutable.
pub(crate) struct ValidatedArchive<T>
where
    T: Archive,
    T::Archived: Portable,
{
    bytes: AlignedVec,
    _root: PhantomData<fn() -> T>,
}

impl<T> fmt::Debug for ValidatedArchive<T>
where
    T: Archive,
    T::Archived: Portable,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ValidatedArchive")
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

impl<T> ValidatedArchive<T>
where
    T: Archive,
    T::Archived: Portable,
{
    pub(crate) fn root(&self) -> &Archived<T> {
        let bytes = self.bytes.as_slice();
        // SAFETY: `freeze` serialized exactly one `T`, validated the exact
        // archived root at the default position, and retained the same aligned
        // allocation. This owner exposes no mutable bytes, so the validated
        // representation cannot change. The returned reference is tied to
        // `&self`, which owns the allocation for at least this lifetime.
        unsafe { rkyv::access_unchecked::<Archived<T>>(bytes) }
    }

    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.bytes.capacity()
    }
}

impl<T> ValidatedArchive<T>
where
    T: Archive,
    T::Archived: Portable + for<'a> CheckBytes<rkyv::api::high::HighValidator<'a, Error>>,
    T: for<'a> Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, Error>>,
{
    pub(crate) fn freeze(value: &T) -> Result<Arc<Self>, String> {
        #[cfg(test)]
        if FAIL_NEXT_FREEZE.with(|fail| fail.replace(false)) {
            return Err("injected archive-finalization failure".into());
        }
        let mut bytes = rkyv::to_bytes::<Error>(value)
            .map_err(|error| format!("archive serialization failed: {error}"))?;
        if SHRINK_OVERALLOCATED_ARCHIVES
            && bytes.len() >= 64 * 1024
            && bytes.capacity().saturating_mul(4) >= bytes.len().saturating_mul(5)
        {
            bytes.shrink_to_fit();
        }
        let _validated = access::<Archived<T>, Error>(&bytes)
            .map_err(|error| format!("archive validation failed: {error}"))?;
        let owner = Arc::new(Self {
            bytes,
            _root: PhantomData,
        });
        StorageWorkCounters::update(|counters| {
            counters.archive_bytes_generated = counters
                .archive_bytes_generated
                .saturating_add(owner.len() as u64);
        });
        Ok(owner)
    }

}

#[cfg(test)]
impl<T> ValidatedArchive<T>
where
    T: Archive,
    T::Archived: Portable + for<'a> CheckBytes<rkyv::api::high::HighValidator<'a, Error>>,
{
    pub(crate) fn checked_from_bytes(bytes: Vec<u8>) -> Result<Arc<Self>, String> {
        let mut aligned = AlignedVec::with_capacity(bytes.len());
        aligned.extend_from_slice(&bytes);
        access::<Archived<T>, Error>(&aligned)
            .map_err(|error| format!("archive validation failed: {error}"))?;
        Ok(Arc::new(Self {
            bytes: aligned,
            _root: PhantomData,
        }))
    }
}

#[cfg(test)]
mod tests {
    use rkyv::{Archive, Deserialize, Serialize};

    use super::ValidatedArchive;

    #[derive(Archive, Deserialize, Serialize, Debug, Eq, PartialEq)]
    struct Fixture {
        label: String,
        values: Vec<u32>,
    }

    #[test]
    fn validated_archive_owns_and_borrows_checked_bytes() {
        let owner = ValidatedArchive::freeze(&Fixture {
            label: "stable".into(),
            values: vec![1, 2, 3],
        })
        .unwrap();
        assert_eq!(owner.root().label, "stable");
        assert_eq!(owner.root().values.len(), 3);
        let shared = owner.clone();
        assert!(std::ptr::eq(
            shared.root() as *const _,
            owner.root() as *const _
        ));
        assert!(owner.capacity() >= owner.len());
    }

    #[test]
    fn checked_test_constructor_rejects_truncated_archive() {
        let owner =
            ValidatedArchive::freeze(&Fixture { label: "short".into(), values: vec![7] }).unwrap();
        let mut bytes = owner.bytes.to_vec();
        bytes.truncate(bytes.len() / 2);
        assert!(ValidatedArchive::<Fixture>::checked_from_bytes(bytes).is_err());
    }
}
