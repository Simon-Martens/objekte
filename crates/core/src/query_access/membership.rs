//! Domain-qualified query membership with explicit ownership.

use std::borrow::Cow;
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
            (_, Self::Empty) => false,
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
    Sorted(std::iter::Copied<std::slice::Iter<'a, u32>>),
}

impl Iterator for MembershipIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::Bitmap(iter) => iter.next(),
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

    fn concrete(&self) -> Option<&RoaringBitmap> {
        match self {
            Self::Borrowed(bitmap) => Some(bitmap),
            Self::Owned(bitmap) => Some(bitmap),
            Self::Shared(bitmap) => Some(bitmap),
            Self::Unrestricted | Self::Empty | Self::Sorted(_) => None,
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
            match &slots {
                SlotSet::Sorted(slots) => slots.iter().all(|&slot| input.contains(slot)),
                _ => slots
                    .concrete()
                    .is_none_or(|members| members.is_subset(input)),
            },
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
            SlotSet::Sorted(slots) => MembershipView::Sorted(slots),
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
            SlotSet::Empty | SlotSet::Sorted(_) => None,
            SlotSet::Borrowed(bitmap) => Some(bitmap),
            SlotSet::Owned(bitmap) => Some(bitmap),
            SlotSet::Shared(bitmap) => Some(bitmap),
        }
    }

    /// Native kernels can use a computed bitmap when handed an encoded result.
    /// The common native path keeps borrowing the original membership.
    pub(crate) fn bitmap_for_native(&self) -> Cow<'_, RoaringBitmap> {
        if let Some(bitmap) = self.effective_bitmap() {
            Cow::Borrowed(bitmap)
        } else {
            Cow::Owned(self.iter().collect())
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
            SlotSet::Sorted(slots) => slots.iter().copied().collect(),
        }
    }

    pub(crate) fn replace_slots(self, slots: SlotSet<'a>) -> Self {
        if matches!(slots, SlotSet::Unrestricted) {
            self
        } else {
            Self::new(self.domain, self.input, slots)
        }
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
        let mixed = match (&self.slots, &other.slots) {
            (SlotSet::Sorted(slots), _) => Some((slots, &other)),
            (_, SlotSet::Sorted(slots)) => Some((slots, &self)),
            _ => None,
        };
        if let Some((slots, target)) = mixed {
            let result = if slots.iter().all(|&slot| target.contains(slot)) {
                SlotSet::Sorted(Arc::clone(slots))
            } else {
                SlotSet::Owned(
                    slots
                        .iter()
                        .copied()
                        .filter(|&slot| target.contains(slot))
                        .collect(),
                )
            };
            return Ok(Self::new_algebra(self.domain, self.input, result));
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
        if matches!(self.slots, SlotSet::Sorted(_)) || matches!(other.slots, SlotSet::Sorted(_)) {
            let domain = self.domain;
            let input = self.input;
            let (base, added) = if matches!(self.slots, SlotSet::Sorted(_)) {
                (other, self)
            } else {
                (self, other)
            };
            if added.is_subset(&base)? {
                return Ok(base);
            }
            let mut result = base.into_owned_members();
            result.extend(added.iter());
            return Ok(Self::new_algebra(domain, input, SlotSet::Owned(result)));
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
        if let SlotSet::Sorted(slots) = &self.slots {
            if self.is_disjoint(&other)? {
                return Ok(self);
            }
            let remaining = slots
                .iter()
                .copied()
                .filter(|&slot| !other.contains(slot))
                .collect();
            return Ok(Self::new_algebra(
                self.domain,
                self.input,
                SlotSet::Owned(remaining),
            ));
        }
        if let SlotSet::Sorted(slots) = &other.slots {
            if self.is_disjoint(&other)? {
                return Ok(self);
            }
            let domain = self.domain;
            let input = self.input;
            let mut result = self.into_owned_members();
            for &slot in slots.iter() {
                result.remove(slot);
            }
            return Ok(Self::new_algebra(domain, input, SlotSet::Owned(result)));
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
            SlotSet::Empty | SlotSet::Sorted(_) => unreachable!("handled above"),
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
