//! Domain-qualified query membership with explicit ownership.

use std::sync::Arc;

use roaring::RoaringBitmap;

use super::contract::AccessError;

/// Identifies one pinned local slot domain. The number is opaque outside the
/// domain adapter and prevents equal local slot numbers from being combined
/// accidentally across collections, segments or revisions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DomainId(u64);

impl DomainId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// One query result without an implicit deep copy.
///
/// `Unrestricted` is symbolic: it means the supplied input domain. It is not
/// materialized unless an operation genuinely needs a different result.
#[derive(Debug)]
pub(crate) enum SlotSet<'a> {
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

    fn materialized(&self) -> Option<&RoaringBitmap> {
        match self {
            Self::Unrestricted => None,
            Self::Empty => None,
            Self::Borrowed(bitmap) => Some(bitmap),
            Self::Owned(bitmap) => Some(bitmap),
            Self::Shared(bitmap) => Some(bitmap),
        }
    }

    fn is_owned(&self) -> bool {
        matches!(self, Self::Owned(_))
    }
}

/// A membership value paired with the domain that gives its local slot
/// numbers meaning, and with the borrowed input used by symbolic values.
pub(crate) struct SlotInput<'a> {
    domain: DomainId,
    input: &'a RoaringBitmap,
    slots: SlotSet<'a>,
}

impl<'a> SlotInput<'a> {
    pub(crate) fn unrestricted(domain: DomainId, input: &'a RoaringBitmap) -> Self {
        Self {
            domain,
            input,
            slots: SlotSet::Unrestricted,
        }
    }

    pub(crate) fn new(domain: DomainId, input: &'a RoaringBitmap, slots: SlotSet<'a>) -> Self {
        Self {
            domain,
            input,
            slots,
        }
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    pub(crate) fn input(&self) -> &'a RoaringBitmap {
        self.input
    }

    pub(crate) fn slots(&self) -> &SlotSet<'a> {
        &self.slots
    }

    pub(crate) fn into_slots(self) -> SlotSet<'a> {
        self.slots
    }

    fn bitmap(&self) -> Result<Option<&RoaringBitmap>, AccessError> {
        Ok(match &self.slots {
            SlotSet::Unrestricted => Some(self.input),
            SlotSet::Empty => None,
            SlotSet::Borrowed(bitmap) => Some(bitmap),
            SlotSet::Owned(bitmap) => Some(bitmap),
            SlotSet::Shared(bitmap) => Some(bitmap),
        })
    }

    pub(crate) fn len(&self) -> usize {
        match &self.slots {
            SlotSet::Empty => 0,
            SlotSet::Unrestricted => self.input.len() as usize,
            SlotSet::Borrowed(bitmap) => bitmap.len() as usize,
            SlotSet::Owned(bitmap) => bitmap.len() as usize,
            SlotSet::Shared(bitmap) => bitmap.len() as usize,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        match &self.slots {
            SlotSet::Empty => true,
            SlotSet::Unrestricted => self.input.is_empty(),
            SlotSet::Borrowed(bitmap) => bitmap.is_empty(),
            SlotSet::Owned(bitmap) => bitmap.is_empty(),
            SlotSet::Shared(bitmap) => bitmap.is_empty(),
        }
    }

    pub(crate) fn contains(&self, slot: u32) -> bool {
        match &self.slots {
            SlotSet::Empty => false,
            SlotSet::Unrestricted => self.input.contains(slot),
            SlotSet::Borrowed(bitmap) => bitmap.contains(slot),
            SlotSet::Owned(bitmap) => bitmap.contains(slot),
            SlotSet::Shared(bitmap) => bitmap.contains(slot),
        }
    }

    pub(crate) fn iter(&self) -> Box<dyn Iterator<Item = u32> + '_> {
        match &self.slots {
            SlotSet::Empty => Box::new(std::iter::empty()),
            SlotSet::Unrestricted => Box::new(self.input.iter()),
            SlotSet::Borrowed(bitmap) => Box::new(bitmap.iter()),
            SlotSet::Owned(bitmap) => Box::new(bitmap.iter()),
            SlotSet::Shared(bitmap) => Box::new(bitmap.iter()),
        }
    }

    pub(crate) fn same_members(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        Ok(match (self.bitmap()?, other.bitmap()?) {
            (None, None) => self.input.is_empty() && other.input.is_empty(),
            (None, Some(bitmap)) => self.input == bitmap,
            (Some(bitmap), None) => bitmap == other.input,
            (Some(left), Some(right)) => left == right,
        })
    }

    pub(crate) fn is_disjoint(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        Ok(match (self.bitmap()?, other.bitmap()?) {
            (None, _) | (_, None) => true,
            (Some(left), Some(right)) => left.is_disjoint(right),
        })
    }

    pub(crate) fn is_subset(&self, other: &Self) -> Result<bool, AccessError> {
        self.ensure_same_domain(other)?;
        Ok(match (self.bitmap()?, other.bitmap()?) {
            (None, None) => self.input.is_subset(other.input),
            (None, Some(bitmap)) => self.input.is_subset(bitmap),
            (Some(_), None) => false,
            (Some(left), Some(right)) => left.is_subset(right),
        })
    }

    pub(crate) fn intersection_len(&self, other: &Self) -> Result<u64, AccessError> {
        self.ensure_same_domain(other)?;
        Ok(match (self.bitmap()?, other.bitmap()?) {
            (None, None) => self.input.intersection_len(other.input),
            (None, Some(bitmap)) => self.input.intersection_len(bitmap),
            (Some(bitmap), None) => bitmap.intersection_len(other.input),
            (Some(left), Some(right)) => left.intersection_len(right),
        })
    }

    fn ensure_same_domain(&self, other: &Self) -> Result<(), AccessError> {
        if self.domain == other.domain {
            Ok(())
        } else {
            Err(AccessError::DomainMismatch)
        }
    }

    /// Intersection that forwards identities and mutates an exclusively owned
    /// operand. A genuinely new result allocates exactly one destination.
    pub(crate) fn intersect(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        if self.is_empty() || other.is_empty() {
            return Ok(Self::new(self.domain, self.input, SlotSet::Empty));
        }
        if matches!(self.slots, SlotSet::Unrestricted) {
            return self.forward_slots(other);
        }
        if matches!(other.slots, SlotSet::Unrestricted) {
            return other.forward_slots(self);
        }
        let other_bitmap = other
            .bitmap()?
            .ok_or_else(|| AccessError::Internal("empty membership lost".into()))?;
        let slots = match self.slots {
            SlotSet::Owned(mut bitmap) => {
                bitmap &= other_bitmap;
                SlotSet::Owned(bitmap)
            }
            SlotSet::Borrowed(bitmap) => SlotSet::Owned(bitmap & other_bitmap),
            SlotSet::Shared(bitmap) => SlotSet::Owned(bitmap.as_ref() & other_bitmap),
            SlotSet::Unrestricted | SlotSet::Empty => unreachable!("identity handled above"),
        };
        Ok(Self::new(self.domain, self.input, slots))
    }

    /// Union with the same ownership rules as intersection.
    pub(crate) fn union(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        if matches!(self.slots, SlotSet::Unrestricted) {
            return Ok(Self::new(self.domain, self.input, SlotSet::Unrestricted));
        }
        if matches!(other.slots, SlotSet::Unrestricted) {
            return Ok(Self::new(self.domain, self.input, SlotSet::Unrestricted));
        }
        if self.is_empty() {
            return self.forward_slots(other);
        }
        if other.is_empty() {
            return other.forward_slots(self);
        }
        let other_bitmap = other
            .bitmap()?
            .ok_or_else(|| AccessError::Internal("empty membership lost".into()))?;
        let slots = match self.slots {
            SlotSet::Owned(mut bitmap) => {
                bitmap |= other_bitmap;
                SlotSet::Owned(bitmap)
            }
            SlotSet::Borrowed(bitmap) => SlotSet::Owned(bitmap | other_bitmap),
            SlotSet::Shared(bitmap) => SlotSet::Owned(bitmap.as_ref() | other_bitmap),
            SlotSet::Unrestricted | SlotSet::Empty => unreachable!("identity handled above"),
        };
        Ok(Self::new(self.domain, self.input, slots))
    }

    /// Subtraction from a supplied input. The owned left operand is updated in
    /// place; borrowed operands produce only the mathematical destination.
    pub(crate) fn subtract(self, other: Self) -> Result<Self, AccessError> {
        self.ensure_same_domain(&other)?;
        if matches!(other.slots, SlotSet::Empty) {
            return Ok(self);
        }
        if matches!(self.slots, SlotSet::Empty) {
            return Ok(self);
        }
        if matches!(other.slots, SlotSet::Unrestricted) {
            return Ok(Self::new(self.domain, self.input, SlotSet::Empty));
        }
        let other_bitmap = other
            .bitmap()?
            .ok_or_else(|| AccessError::Internal("empty membership lost".into()))?;
        let slots = match self.slots {
            SlotSet::Unrestricted => SlotSet::Owned(self.input - other_bitmap),
            SlotSet::Owned(mut bitmap) => {
                bitmap -= other_bitmap;
                SlotSet::Owned(bitmap)
            }
            SlotSet::Borrowed(bitmap) => SlotSet::Owned(bitmap - other_bitmap),
            SlotSet::Shared(bitmap) => SlotSet::Owned(bitmap.as_ref() - other_bitmap),
            SlotSet::Empty => unreachable!("empty handled above"),
        };
        Ok(Self::new(self.domain, self.input, slots))
    }

    fn forward_slots(self, other: Self) -> Result<Self, AccessError> {
        let domain = self.domain;
        let input = if self.input.len() >= other.input.len() {
            self.input
        } else {
            other.input
        };
        Ok(Self::new(domain, input, other.slots))
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
