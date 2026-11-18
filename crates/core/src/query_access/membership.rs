//! Domain-qualified query membership with explicit ownership.

use std::sync::Arc;

use roaring::RoaringBitmap;
use roaring::bitmap::Iter as RoaringIter;

use super::contract::AccessError;

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
}

impl<'a> MembershipView<'a> {
    pub(crate) fn len(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => bitmap.len() as usize,
        }
    }

    pub(crate) fn is_empty(self) -> bool {
        match self {
            Self::Empty => true,
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => bitmap.is_empty(),
        }
    }

    pub(crate) fn contains(self, slot: u32) -> bool {
        match self {
            Self::Empty => false,
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => bitmap.contains(slot),
        }
    }

    pub(crate) fn iter(self) -> MembershipIter<'a> {
        match self {
            Self::Empty => MembershipIter::Empty,
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => {
                MembershipIter::Bitmap(bitmap.iter())
            }
        }
    }

    fn bitmap(self) -> Option<&'a RoaringBitmap> {
        match self {
            Self::Empty => None,
            Self::AllSlots(bitmap) | Self::Borrowed(bitmap) => Some(bitmap),
        }
    }

    pub(crate) fn same_members(self, other: Self) -> bool {
        match (self, other) {
            (Self::Empty, Self::Empty) => true,
            (Self::Empty, _) | (_, Self::Empty) => false,
            (left, right) => left.bitmap() == right.bitmap(),
        }
    }

    pub(crate) fn is_disjoint(self, other: Self) -> bool {
        match (self, other) {
            (Self::Empty, _) | (_, Self::Empty) => true,
            (left, right) => left
                .bitmap()
                .zip(right.bitmap())
                .is_some_and(|(left, right)| left.is_disjoint(right)),
        }
    }

    pub(crate) fn is_subset(self, other: Self) -> bool {
        match (self, other) {
            (Self::Empty, _) => true,
            (_, Self::Empty) => false,
            (left, right) => left
                .bitmap()
                .zip(right.bitmap())
                .is_some_and(|(left, right)| left.is_subset(right)),
        }
    }

    pub(crate) fn intersection_len(self, other: Self) -> u64 {
        match (self, other) {
            (Self::Empty, _) | (_, Self::Empty) => 0,
            (left, right) => left
                .bitmap()
                .zip(right.bitmap())
                .map_or(0, |(left, right)| left.intersection_len(right)),
        }
    }
}

/// Unboxed iterator over effective members.
pub(crate) enum MembershipIter<'a> {
    Empty,
    Bitmap(RoaringIter<'a>),
}

impl Iterator for MembershipIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::Bitmap(iter) => iter.next(),
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

    fn concrete(&self) -> Option<&RoaringBitmap> {
        match self {
            Self::Borrowed(bitmap) => Some(bitmap),
            Self::Owned(bitmap) => Some(bitmap),
            Self::Shared(bitmap) => Some(bitmap),
            Self::Unrestricted | Self::Empty => None,
        }
    }
}

/// A result paired with its pinned domain and the exact operation input.
pub(crate) struct SlotInput<'a> {
    domain: DomainId,
    input: &'a RoaringBitmap,
    slots: SlotSet<'a>,
}

impl<'a> SlotInput<'a> {
    pub(crate) fn unrestricted(domain: DomainId, input: &'a RoaringBitmap) -> Self {
        Self::new(domain, input, SlotSet::Unrestricted)
    }

    pub(crate) fn empty(domain: DomainId, input: &'a RoaringBitmap) -> Self {
        Self::new(domain, input, SlotSet::Empty)
    }

    pub(crate) fn new(domain: DomainId, input: &'a RoaringBitmap, slots: SlotSet<'a>) -> Self {
        debug_assert!(
            slots
                .concrete()
                .is_none_or(|members| members.is_subset(input)),
            "effective members must be contained by the supplied input"
        );
        Self {
            domain,
            input,
            slots,
        }
    }

    fn new_algebra(domain: DomainId, input: &'a RoaringBitmap, slots: SlotSet<'a>) -> Self {
        Self {
            domain,
            input,
            slots,
        }
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    pub(crate) fn effective(&self) -> MembershipView<'_> {
        match &self.slots {
            SlotSet::Unrestricted => {
                if self.input.is_empty() {
                    MembershipView::Empty
                } else {
                    MembershipView::AllSlots(self.input)
                }
            }
            SlotSet::Empty => MembershipView::Empty,
            SlotSet::Borrowed(bitmap) => {
                if bitmap.is_empty() {
                    MembershipView::Empty
                } else {
                    MembershipView::Borrowed(bitmap)
                }
            }
            SlotSet::Owned(bitmap) => {
                if bitmap.is_empty() {
                    MembershipView::Empty
                } else {
                    MembershipView::Borrowed(bitmap)
                }
            }
            SlotSet::Shared(bitmap) => {
                if bitmap.is_empty() {
                    MembershipView::Empty
                } else {
                    MembershipView::Borrowed(bitmap)
                }
            }
        }
    }

    pub(crate) fn effective_bitmap(&self) -> Option<&RoaringBitmap> {
        match &self.slots {
            SlotSet::Unrestricted => Some(self.input),
            SlotSet::Empty => None,
            SlotSet::Borrowed(bitmap) => Some(bitmap),
            SlotSet::Owned(bitmap) => Some(bitmap),
            SlotSet::Shared(bitmap) => Some(bitmap),
        }
    }

    pub(crate) fn operation_input(&self) -> &'a RoaringBitmap {
        self.input
    }

    pub(crate) fn slots(&self) -> &SlotSet<'a> {
        &self.slots
    }

    pub(crate) fn into_slots(self) -> SlotSet<'a> {
        self.slots
    }

    pub(crate) fn into_owned_members(self) -> RoaringBitmap {
        match self.slots {
            SlotSet::Unrestricted => self.input.clone(),
            SlotSet::Empty => RoaringBitmap::new(),
            SlotSet::Borrowed(bitmap) => bitmap.clone(),
            SlotSet::Owned(bitmap) => bitmap,
            SlotSet::Shared(bitmap) => bitmap.as_ref().clone(),
        }
    }

    pub(crate) fn replace_slots(self, slots: SlotSet<'a>) -> Self {
        Self::new(self.domain, self.input, slots)
    }

    pub(crate) fn len(&self) -> usize {
        self.effective().len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.effective().is_empty()
    }

    pub(crate) fn contains(&self, slot: u32) -> bool {
        self.effective().contains(slot)
    }

    pub(crate) fn iter(&self) -> MembershipIter<'_> {
        self.effective().iter()
    }

    pub(crate) fn same_members(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        Ok(self.effective().same_members(other.effective()))
    }

    pub(crate) fn is_disjoint(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        Ok(self.effective().is_disjoint(other.effective()))
    }

    pub(crate) fn is_subset(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        Ok(self.effective().is_subset(other.effective()))
    }

    pub(crate) fn intersection_len(&self, other: &Self) -> Result<u64, AccessError> {
        self.ensure_same_domain(other)?;
        Ok(self.effective().intersection_len(other.effective()))
    }

    fn ensure_same_domain(&self, other: &Self) -> Result<(), AccessError> {
        if self.domain == other.domain {
            Ok(())
        } else {
            Err(AccessError::DomainMismatch)
        }
    }

    /// Intersection forwards unchanged operands and mutates an exclusively
    /// owned left operand. The output input handle always comes from the left
    /// operand; symbolic membership never acquires another operand's input.
    pub(crate) fn intersect(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        if self.is_empty() {
            return Ok(Self::new_algebra(self.domain, self.input, SlotSet::Empty));
        }
        if other.is_empty() {
            return Ok(Self::new_algebra(self.domain, self.input, SlotSet::Empty));
        }
        match (self.slots, other.slots) {
            (SlotSet::Unrestricted, SlotSet::Unrestricted) => {
                if std::ptr::eq(self.input, other.input) {
                    Ok(Self::new_algebra(
                        self.domain,
                        self.input,
                        SlotSet::Unrestricted,
                    ))
                } else {
                    Ok(Self::new_algebra(
                        self.domain,
                        self.input,
                        SlotSet::Owned(self.input & other.input),
                    ))
                }
            }
            (SlotSet::Unrestricted, slots) => {
                let members = slots
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing intersection operand".into()))?;
                if members.is_subset(self.input) {
                    Ok(Self::new_algebra(self.domain, self.input, slots))
                } else {
                    Ok(Self::new_algebra(
                        self.domain,
                        self.input,
                        SlotSet::Owned(self.input & members),
                    ))
                }
            }
            (slots, SlotSet::Unrestricted) => {
                let members = slots
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing intersection operand".into()))?;
                if members.is_subset(other.input) {
                    Ok(Self::new_algebra(self.domain, self.input, slots))
                } else {
                    Ok(Self::new_algebra(
                        self.domain,
                        self.input,
                        SlotSet::Owned(members & other.input),
                    ))
                }
            }
            (SlotSet::Owned(mut bitmap), other) => {
                bitmap &= other
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing intersection operand".into()))?;
                Ok(Self::new_algebra(
                    self.domain,
                    self.input,
                    SlotSet::Owned(bitmap),
                ))
            }
            (slots, other) => {
                let left = slots
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing intersection operand".into()))?;
                let right = other
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing intersection operand".into()))?;
                Ok(Self::new_algebra(
                    self.domain,
                    self.input,
                    SlotSet::Owned(left & right),
                ))
            }
        }
    }

    /// Union forwards full-range identity and mutates an exclusively owned left
    /// operand where possible.
    pub(crate) fn union(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        match (&self.slots, &other.slots) {
            (SlotSet::Unrestricted, SlotSet::Unrestricted) => {
                return if std::ptr::eq(self.input, other.input) {
                    Ok(self)
                } else {
                    Ok(Self::new_algebra(
                        self.domain,
                        self.input,
                        SlotSet::Owned(self.input | other.input),
                    ))
                };
            }
            (SlotSet::Unrestricted, other_slots) => {
                if let Some(members) = other_slots.concrete() {
                    return if members.is_subset(self.input) {
                        Ok(self)
                    } else {
                        Ok(Self::new_algebra(
                            self.domain,
                            self.input,
                            SlotSet::Owned(self.input | members),
                        ))
                    };
                }
            }
            (self_slots, SlotSet::Unrestricted) => {
                if let Some(members) = self_slots.concrete() {
                    return if members.is_subset(other.input) {
                        Ok(other)
                    } else {
                        Ok(Self::new_algebra(
                            other.domain,
                            other.input,
                            SlotSet::Owned(members | other.input),
                        ))
                    };
                }
            }
            _ => {}
        }
        if self.is_empty() {
            return Ok(other);
        }
        if other.is_empty() {
            return Ok(self);
        }
        match (self.slots, other.slots) {
            (SlotSet::Owned(mut bitmap), other) => {
                bitmap |= other
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing union operand".into()))?;
                Ok(Self::new_algebra(
                    self.domain,
                    self.input,
                    SlotSet::Owned(bitmap),
                ))
            }
            (slots, SlotSet::Owned(mut bitmap)) => {
                bitmap |= slots
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing union operand".into()))?;
                Ok(Self::new_algebra(
                    self.domain,
                    self.input,
                    SlotSet::Owned(bitmap),
                ))
            }
            (slots, other) => {
                let left = slots
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing union operand".into()))?;
                let right = other
                    .concrete()
                    .ok_or_else(|| AccessError::Internal("missing union operand".into()))?;
                Ok(Self::new_algebra(
                    self.domain,
                    self.input,
                    SlotSet::Owned(left | right),
                ))
            }
        }
    }

    /// Subtraction preserves left-to-right operand meaning and mutates an
    /// exclusively owned left result in place.
    pub(crate) fn subtract(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        if self.is_empty() || matches!(other.slots, SlotSet::Empty) {
            return Ok(self);
        }
        if matches!(self.slots, SlotSet::Unrestricted)
            && matches!(other.slots, SlotSet::Unrestricted)
        {
            return Ok(Self::new_algebra(
                self.domain,
                self.input,
                if std::ptr::eq(self.input, other.input) {
                    SlotSet::Empty
                } else {
                    SlotSet::Owned(self.input - other.input)
                },
            ));
        }
        if matches!(other.slots, SlotSet::Unrestricted) {
            let members = self
                .effective_bitmap()
                .ok_or_else(|| AccessError::Internal("missing subtraction operand".into()))?;
            return if members.is_disjoint(other.input) {
                Ok(self)
            } else {
                Ok(Self::new_algebra(
                    self.domain,
                    self.input,
                    SlotSet::Owned(members - other.input),
                ))
            };
        }
        let right = other
            .slots
            .concrete()
            .ok_or_else(|| AccessError::Internal("missing subtraction operand".into()))?;
        match self.slots {
            SlotSet::Unrestricted => Ok(Self::new_algebra(
                self.domain,
                self.input,
                SlotSet::Owned(self.input - right),
            )),
            SlotSet::Owned(mut bitmap) => {
                bitmap -= right;
                Ok(Self::new_algebra(
                    self.domain,
                    self.input,
                    SlotSet::Owned(bitmap),
                ))
            }
            SlotSet::Borrowed(bitmap) => Ok(Self::new_algebra(
                self.domain,
                self.input,
                SlotSet::Owned(bitmap - right),
            )),
            SlotSet::Shared(bitmap) => Ok(Self::new_algebra(
                self.domain,
                self.input,
                SlotSet::Owned(bitmap.as_ref() - right),
            )),
            SlotSet::Empty => unreachable!("empty handled above"),
        }
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
