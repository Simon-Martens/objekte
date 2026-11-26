//! Domain-qualified query membership with explicit ownership.

mod segments;
pub(crate) use segments::SegmentRecordSet;

use std::borrow::Cow;
use std::sync::Arc;

use roaring::RoaringBitmap;
use roaring::bitmap::Iter as RoaringIter;

use super::contract::AccessError;
use crate::index::storage::liveness::ActiveRecords;
use crate::model::RecordSlot;

/// The storage owner of a pinned local slot domain.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DomainSource {
    Base,
    Segment(u64),
}

/// Complete identity of one pinned local slot domain. Every component remains
/// at its full width; no hash or truncation is used to make local slot numbers
/// comparable across repositories, revisions, collections or segments.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DomainId {
    repository: u64,
    collection: u32,
    revision: u64,
    source: DomainSource,
}

impl DomainId {
    /// Synthetic opaque identity for isolated unit fixtures.
    pub(crate) const fn new(value: u64) -> Self {
        Self {
            repository: value,
            collection: 0,
            revision: 0,
            source: DomainSource::Base,
        }
    }

    pub(crate) const fn from_parts(
        repository: u64,
        collection: u32,
        revision: u64,
        segment: Option<u64>,
    ) -> Self {
        Self {
            repository,
            collection,
            revision,
            source: match segment {
                Some(segment) => DomainSource::Segment(segment),
                None => DomainSource::Base,
            },
        }
    }
}

/// Read-only membership inside a pinned domain.
#[derive(Clone, Copy, Debug)]
pub(crate) enum MembershipView<'a> {
    Empty,
    AllSlots(&'a RoaringBitmap),
    Borrowed(&'a RoaringBitmap),
    Sorted(&'a [u32]),
}

impl<'a> MembershipView<'a> {
    pub(crate) fn len(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Sorted(slots) => slots.len(),
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => bitmap.len() as usize,
        }
    }

    pub(crate) fn is_empty(self) -> bool {
        match self {
            Self::Empty => true,
            Self::Sorted(slots) => slots.is_empty(),
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => bitmap.is_empty(),
        }
    }

    pub(crate) fn contains(self, slot: u32) -> bool {
        match self {
            Self::Empty => false,
            Self::Sorted(slots) => slots.binary_search(&slot).is_ok(),
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => bitmap.contains(slot),
        }
    }

    pub(crate) fn last(self) -> Option<u32> {
        match self {
            Self::Empty => None,
            Self::Sorted(slots) => slots.last().copied(),
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => bitmap.max(),
        }
    }

    pub(crate) fn iter(self) -> MembershipIter<'a> {
        match self {
            Self::Empty => MembershipIter::Empty,
            Self::Sorted(slots) => MembershipIter::Sorted(slots.iter().copied()),
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => {
                MembershipIter::Bitmap(bitmap.iter())
            }
        }
    }

    fn bitmap(self) -> Option<&'a RoaringBitmap> {
        match self {
            Self::Empty | Self::Sorted(_) => None,
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => Some(bitmap),
        }
    }

    pub(crate) fn bitmap_for_set_algebra(self) -> std::borrow::Cow<'a, RoaringBitmap> {
        match self.bitmap() {
            Some(bitmap) => std::borrow::Cow::Borrowed(bitmap),
            None => std::borrow::Cow::Owned(self.iter().collect()),
        }
    }

    pub(crate) fn same_members(self, other: Self) -> bool {
        match (self, other) {
            (Self::Sorted(left), Self::Sorted(right)) => left == right,
            (left, right) => match (left.bitmap(), right.bitmap()) {
                (Some(left), Some(right)) => left == right,
                _ => left.len() == right.len() && left.is_subset(right),
            },
        }
    }

    pub(crate) fn is_disjoint(self, other: Self) -> bool {
        match (self.bitmap(), other.bitmap()) {
            (Some(left), Some(right)) => left.is_disjoint(right),
            _ => !self.any_in_common(other),
        }
    }

    fn any_in_common(self, other: Self) -> bool {
        match (self, other) {
            (Self::Sorted(slots), _) | (_, Self::Sorted(slots)) => {
                let target = if matches!(self, Self::Sorted(_)) {
                    other
                } else {
                    self
                };
                slots.iter().any(|&slot| target.contains(slot))
            }
            _ => false,
        }
    }

    pub(crate) fn is_subset(self, other: Self) -> bool {
        match (self, other) {
            (Self::Empty, _) => true,
            (left, Self::Empty) => left.is_empty(),
            (Self::Sorted(slots), right) => slots.iter().all(|&slot| right.contains(slot)),
            (left, Self::Sorted(slots)) => {
                left.len() <= slots.len() && left.iter().all(|slot| other.contains(slot))
            }
            (
                Self::AllSlots(left) | Self::Borrowed(left),
                Self::AllSlots(right) | Self::Borrowed(right),
            ) => left.is_subset(right),
        }
    }

    pub(crate) fn union_into_bitmap(self, output: &mut RoaringBitmap) {
        match self {
            Self::Empty => {}
            Self::Borrowed(slots) | Self::AllSlots(slots) => *output |= slots,
            Self::Sorted(slots) => output.extend(slots.iter().copied()),
        }
    }

    pub(crate) fn intersect_bitmap(self, input: &RoaringBitmap) -> RoaringBitmap {
        match self {
            Self::Empty => RoaringBitmap::new(),
            Self::Sorted(slots) => slots
                .iter()
                .copied()
                .filter(|slot| input.contains(*slot))
                .collect(),
            Self::Borrowed(slots) | Self::AllSlots(slots) => input & slots,
        }
    }

    pub(crate) fn subtract_from(self, input: &RoaringBitmap) -> RoaringBitmap {
        match self {
            Self::Empty => input.clone(),
            Self::Sorted(_) => input.iter().filter(|&slot| !self.contains(slot)).collect(),
            Self::Borrowed(slots) | Self::AllSlots(slots) => input - slots,
        }
    }

    pub(crate) fn intersection_len(self, other: Self) -> u64 {
        match (self, other) {
            (Self::Empty, _) | (_, Self::Empty) => 0,
            (Self::Sorted(slots), right) => {
                slots.iter().filter(|&&slot| right.contains(slot)).count() as u64
            }
            (left, Self::Sorted(slots)) => {
                slots.iter().filter(|&&slot| left.contains(slot)).count() as u64
            }
            (
                Self::AllSlots(left) | Self::Borrowed(left),
                Self::AllSlots(right) | Self::Borrowed(right),
            ) => left.intersection_len(right),
        }
    }
}

/// Unboxed iterator over effective members.
pub(crate) enum MembershipIter<'a> {
    Empty,
    Bitmap(RoaringIter<'a>),
    Owned(roaring::bitmap::IntoIter),
    Sorted(std::iter::Copied<std::slice::Iter<'a, u32>>),
}

impl Iterator for MembershipIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::Bitmap(iter) => iter.next(),
            Self::Owned(iter) => iter.next(),
            Self::Sorted(iter) => iter.next(),
        }
    }
}

/// One query result without an implicit deep copy.
#[derive(Debug)]
pub(crate) enum SlotSet<'a> {
    /// Forward the exact input handle supplied to the operation.
    Unrestricted,
    Empty,
    Borrowed(&'a RoaringBitmap),
    Owned(RoaringBitmap),
    Shared(Arc<RoaringBitmap>),
    Sorted(Arc<[u32]>),
}

impl<'a> SlotSet<'a> {
    pub(crate) fn borrowed(bitmap: &'a RoaringBitmap) -> Self {
        Self::Borrowed(bitmap)
    }

    pub(crate) fn from_owned(bitmap: RoaringBitmap) -> Self {
        Self::Owned(bitmap)
    }

    pub(crate) fn from_shared(bitmap: Arc<RoaringBitmap>) -> Self {
        Self::Shared(bitmap)
    }

    pub(crate) fn from_sorted(slots: Arc<[u32]>) -> Result<Self, AccessError> {
        if slots.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(AccessError::Corrupt(
                "membership slots are not strictly ordered".into(),
            ));
        }
        Ok(if slots.is_empty() {
            Self::Empty
        } else {
            Self::Sorted(slots)
        })
    }

    pub(crate) fn explicit_view(&self) -> Option<MembershipView<'_>> {
        match self {
            Self::Unrestricted => None,
            Self::Empty => Some(MembershipView::Empty),
            Self::Sorted(slots) => Some(MembershipView::Sorted(slots)),
            _ => self.concrete().map(|bitmap| {
                if bitmap.is_empty() {
                    MembershipView::Empty
                } else {
                    MembershipView::Borrowed(bitmap)
                }
            }),
        }
    }

    fn concrete(&self) -> Option<&RoaringBitmap> {
        match self {
            Self::Borrowed(bitmap) => Some(bitmap),
            Self::Owned(bitmap) => Some(bitmap),
            Self::Shared(bitmap) => Some(bitmap),
            Self::Unrestricted | Self::Empty | Self::Sorted(_) => None,
        }
    }
}

/// The operation's pinned or query-owned input membership.
#[derive(Clone, Copy, Debug)]
pub(crate) enum OperationInput<'a> {
    Bitmap(&'a RoaringBitmap),
    Pinned(&'a ActiveRecords),
}

impl<'a> From<&'a RoaringBitmap> for OperationInput<'a> {
    fn from(bitmap: &'a RoaringBitmap) -> Self {
        Self::Bitmap(bitmap)
    }
}

impl<'a> OperationInput<'a> {
    fn resident(self) -> Option<&'a RoaringBitmap> {
        match self {
            Self::Bitmap(bitmap) => Some(bitmap),
            Self::Pinned(active) => active.borrowed_bitmap(),
        }
    }

    fn same_handle(self, other: Self) -> bool {
        match (self, other) {
            (Self::Bitmap(a), Self::Bitmap(b)) => std::ptr::eq(a, b),
            (Self::Pinned(a), Self::Pinned(b)) => std::ptr::eq(a, b),
            _ => false,
        }
    }

    pub(crate) fn bitmap(self) -> Result<Cow<'a, RoaringBitmap>, AccessError> {
        if let Some(bitmap) = self.resident() {
            return Ok(Cow::Borrowed(bitmap));
        }
        match self {
            Self::Pinned(active) => Ok(Cow::Owned(active.bitmap()?)),
            Self::Bitmap(_) => unreachable!("query bitmap is resident"),
        }
    }

    pub(crate) fn len(self) -> usize {
        match self {
            Self::Bitmap(bitmap) => bitmap.len() as usize,
            Self::Pinned(active) => active.len() as usize,
        }
    }

    fn contains(self, slot: u32) -> Result<bool, AccessError> {
        match self {
            Self::Bitmap(bitmap) => Ok(bitmap.contains(slot)),
            Self::Pinned(active) => Ok(active.contains(RecordSlot(slot))?),
        }
    }
}

/// A result paired with its pinned domain and the exact operation input.
pub(crate) struct SlotInput<'a> {
    domain: DomainId,
    input: OperationInput<'a>,
    slots: SlotSet<'a>,
}

impl<'a> SlotInput<'a> {
    pub(crate) fn unrestricted(domain: DomainId, input: impl Into<OperationInput<'a>>) -> Self {
        Self::new(domain, input, SlotSet::Unrestricted)
    }

    pub(crate) fn empty(domain: DomainId, input: impl Into<OperationInput<'a>>) -> Self {
        Self::new(domain, input, SlotSet::Empty)
    }

    pub(crate) fn new(
        domain: DomainId,
        input: impl Into<OperationInput<'a>>,
        slots: SlotSet<'a>,
    ) -> Self {
        let input = input.into();
        debug_assert!(
            input.resident().is_none_or(|input| match &slots {
                SlotSet::Sorted(slots) => slots.iter().all(|&slot| input.contains(slot)),
                _ => slots
                    .concrete()
                    .is_none_or(|members| members.is_subset(input)),
            }),
            "effective members must be contained by the supplied input"
        );
        Self {
            domain,
            input,
            slots,
        }
    }

    fn new_algebra(domain: DomainId, input: OperationInput<'a>, slots: SlotSet<'a>) -> Self {
        Self {
            domain,
            input,
            slots,
        }
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    fn effective_resident(&self) -> Option<MembershipView<'_>> {
        self.slots.explicit_view().or_else(|| {
            self.input.resident().map(|input| {
                if input.is_empty() {
                    MembershipView::Empty
                } else {
                    MembershipView::AllSlots(input)
                }
            })
        })
    }

    pub(crate) fn effective_bitmap(&self) -> Option<&RoaringBitmap> {
        match &self.slots {
            SlotSet::Unrestricted => self.input.resident(),
            _ => self.slots.concrete(),
        }
    }

    /// Native kernels borrow resident membership and materialize only when
    /// required for their bitmap operations.
    pub(crate) fn bitmap_for_native(&self) -> Result<Cow<'_, RoaringBitmap>, AccessError> {
        if let Some(resident) = self.effective_resident() {
            return Ok(resident.bitmap_for_set_algebra());
        }
        self.input.bitmap()
    }

    pub(crate) fn operation_input(&self) -> OperationInput<'a> {
        self.input
    }

    pub(crate) fn slots(&self) -> &SlotSet<'a> {
        &self.slots
    }

    pub(crate) fn into_slots(self) -> SlotSet<'a> {
        self.slots
    }

    pub(crate) fn into_owned_members(self) -> Result<RoaringBitmap, AccessError> {
        Ok(match self.slots {
            SlotSet::Unrestricted => self.input.bitmap()?.into_owned(),
            SlotSet::Empty => RoaringBitmap::new(),
            SlotSet::Borrowed(bitmap) => bitmap.clone(),
            SlotSet::Owned(bitmap) => bitmap,
            SlotSet::Shared(bitmap) => bitmap.as_ref().clone(),
            SlotSet::Sorted(slots) => slots.iter().copied().collect(),
        })
    }

    pub(crate) fn replace_slots(self, slots: SlotSet<'a>) -> Self {
        if matches!(slots, SlotSet::Unrestricted) {
            self
        } else {
            Self::new(self.domain, self.input, slots)
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.slots
            .explicit_view()
            .map_or_else(|| self.input.len(), MembershipView::len)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn contains(&self, slot: u32) -> Result<bool, AccessError> {
        match self.effective_resident() {
            Some(members) => Ok(members.contains(slot)),
            None => self.input.contains(slot),
        }
    }

    pub(crate) fn filter_members(
        &self,
        candidates: impl IntoIterator<Item = u32>,
    ) -> Result<Vec<u32>, AccessError> {
        let mut matched = Vec::new();
        for slot in candidates {
            if self.contains(slot)? {
                matched.push(slot);
            }
        }
        Ok(matched)
    }

    pub(crate) fn iter(&self) -> Result<MembershipIter<'_>, AccessError> {
        if let Some(members) = self.effective_resident() {
            return Ok(members.iter());
        }
        Ok(MembershipIter::Owned(
            self.input.bitmap()?.into_owned().into_iter(),
        ))
    }

    pub(crate) fn same_members(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        if self.is_same_unrestricted(other) {
            return Ok(true);
        }
        Ok(self.len() == other.len() && self.is_subset(other)?)
    }

    pub(crate) fn is_disjoint(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        if self.is_empty() || other.is_empty() {
            return Ok(true);
        }
        if let (Some(left), Some(right)) = (self.effective_resident(), other.effective_resident()) {
            return Ok(left.is_disjoint(right));
        }
        let (first, second) = self.sparse_first(other);
        for slot in first.iter()? {
            if second.contains(slot)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn is_subset(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        if self.len() > other.len() {
            return Ok(false);
        }
        if self.is_same_unrestricted(other) {
            return Ok(true);
        }
        if let (Some(left), Some(right)) = (self.effective_resident(), other.effective_resident()) {
            return Ok(left.is_subset(right));
        }
        for slot in self.iter()? {
            if !other.contains(slot)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub(crate) fn intersection_len(&self, other: &Self) -> Result<u64, AccessError> {
        self.ensure_same_domain(other)?;
        if self.is_same_unrestricted(other) {
            return Ok(self.len() as u64);
        }
        if let (Some(left), Some(right)) = (self.effective_resident(), other.effective_resident()) {
            return Ok(left.intersection_len(right));
        }
        let (first, second) = self.sparse_first(other);
        first.iter()?.try_fold(0, |total, slot| {
            Ok(total + u64::from(second.contains(slot)?))
        })
    }

    fn ensure_same_domain(&self, other: &Self) -> Result<(), AccessError> {
        if self.domain == other.domain {
            Ok(())
        } else {
            Err(AccessError::DomainMismatch)
        }
    }

    fn is_same_unrestricted(&self, other: &Self) -> bool {
        matches!(self.slots, SlotSet::Unrestricted)
            && matches!(other.slots, SlotSet::Unrestricted)
            && self.input.same_handle(other.input)
    }

    fn sparse_first<'b>(&'b self, other: &'b Self) -> (&'b Self, &'b Self) {
        match (self.effective_resident(), other.effective_resident()) {
            (Some(_), None) => (self, other),
            (None, Some(_)) => (other, self),
            _ if self.len() <= other.len() => (self, other),
            _ => (other, self),
        }
    }

    pub(crate) fn intersect(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        if self.is_empty() || other.is_empty() {
            return Ok(Self::new_algebra(self.domain, self.input, SlotSet::Empty));
        }
        if self.is_same_unrestricted(&other) {
            return Ok(self);
        }
        if let (Some(left), Some(right)) = (self.effective_bitmap(), other.effective_bitmap()) {
            if left.len() <= right.len() && left.is_subset(right) {
                return Ok(self);
            }
            if right.len() <= left.len()
                && right.is_subset(left)
                && !matches!(other.slots, SlotSet::Unrestricted)
            {
                return Ok(Self::new_algebra(self.domain, self.input, other.slots));
            }
            let result = match self.slots {
                SlotSet::Owned(mut bitmap) => {
                    bitmap &= right;
                    bitmap
                }
                _ => left & right,
            };
            return Ok(Self::new_algebra(
                self.domain,
                self.input,
                SlotSet::Owned(result),
            ));
        }
        let (first, second) = self.sparse_first(&other);
        let mut result = RoaringBitmap::new();
        for slot in first.iter()? {
            if second.contains(slot)? {
                result.insert(slot);
            }
        }
        Ok(Self::new_algebra(
            self.domain,
            self.input,
            SlotSet::Owned(result),
        ))
    }

    pub(crate) fn union(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        if self.is_empty() || self.is_subset(&other)? {
            return Ok(other);
        }
        if other.is_empty() || other.is_subset(&self)? {
            return Ok(self);
        }
        let domain = self.domain;
        let input = self.input;
        let mut result = self.into_owned_members()?;
        match other.effective_bitmap() {
            Some(bitmap) => result |= bitmap,
            None => result.extend(other.iter()?),
        }
        Ok(Self::new_algebra(domain, input, SlotSet::Owned(result)))
    }

    pub(crate) fn subtract(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        if self.is_empty() || other.is_empty() {
            return Ok(self);
        }
        if self.is_same_unrestricted(&other) {
            return Ok(Self::new_algebra(self.domain, self.input, SlotSet::Empty));
        }
        let domain = self.domain;
        let input = self.input;
        let result = if let Some(right) = other.effective_bitmap() {
            let mut result = self.into_owned_members()?;
            result -= right;
            result
        } else {
            let mut result = RoaringBitmap::new();
            for slot in self.iter()? {
                if !other.contains(slot)? {
                    result.insert(slot);
                }
            }
            result
        };
        Ok(Self::new_algebra(domain, input, SlotSet::Owned(result)))
    }
}

/// A non-clone owner for a result that outlives its source borrow.
#[derive(Debug)]
pub(crate) struct ResultOwner {
    domain: DomainId,
    bitmap: Arc<RoaringBitmap>,
}

impl ResultOwner {
    pub(crate) fn new(domain: DomainId, bitmap: RoaringBitmap) -> Self {
        Self {
            domain,
            bitmap: Arc::new(bitmap),
        }
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    pub(crate) fn share(&self) -> SlotSet<'static> {
        SlotSet::Shared(Arc::clone(&self.bitmap))
    }

    pub(crate) fn len(&self) -> usize {
        self.bitmap.len() as usize
    }
}

#[cfg(test)]
#[path = "membership.test.rs"]
mod tests;
