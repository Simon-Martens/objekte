use std::sync::Arc;

use roaring::RoaringBitmap;

use super::{DomainId, ResultOwner, SlotInput, SlotSet};

fn bitmap(values: &[u32]) -> RoaringBitmap {
    values.iter().copied().collect()
}

#[test]
fn comparisons_are_domain_qualified_and_do_not_materialize() {
    let input = bitmap(&[1, 2, 3, 4]);
    let left_values = Arc::new(bitmap(&[2, 4]));
    let right_values = Arc::new(bitmap(&[2, 3, 4]));
    let left = SlotInput::new(
        DomainId::new(7),
        &input,
        SlotSet::Shared(Arc::clone(&left_values)),
    );
    let right = SlotInput::new(
        DomainId::new(7),
        &input,
        SlotSet::Shared(Arc::clone(&right_values)),
    );
    let pointer = Arc::as_ptr(&left_values);
    assert!(left.is_subset(&right).unwrap());
    assert!(!right.is_subset(&left).unwrap());
    assert!(!left.is_disjoint(&right).unwrap());
    assert_eq!(left.intersection_len(&right).unwrap(), 2);
    assert!(!left.same_members(&right).unwrap());
    assert_eq!(Arc::as_ptr(&left_values), pointer);
    assert_eq!(&*left_values, &bitmap(&[2, 4]));
}

#[test]
fn equal_local_slots_in_different_domains_are_rejected() {
    let input = bitmap(&[1, 2]);
    let left = SlotInput::unrestricted(DomainId::new(1), &input);
    let right = SlotInput::unrestricted(DomainId::new(2), &input);
    assert!(left.same_members(&right).is_err());

    let left = SlotInput::new(DomainId::new(1), &input, SlotSet::from_owned(input.clone()));
    let right = SlotInput::new(DomainId::new(2), &input, SlotSet::from_owned(input.clone()));
    assert!(left.intersect(right).is_err());
}

#[test]
fn set_algebra_forwards_symbolic_identity_and_updates_owned_operands() {
    let input = bitmap(&[1, 2, 3, 4]);
    let restriction_values = Arc::new(bitmap(&[2, 4]));
    let restriction = SlotInput::new(
        DomainId::new(9),
        &input,
        SlotSet::Shared(Arc::clone(&restriction_values)),
    );
    let identity = SlotInput::unrestricted(DomainId::new(9), &input);
    let intersected = restriction.intersect(identity).unwrap();
    assert_eq!(
        intersected.slots().materialized().unwrap(),
        &*restriction_values
    );

    let owned_values = bitmap(&[1, 2, 4]);
    let owned = SlotInput::new(DomainId::new(9), &input, SlotSet::from_owned(owned_values));
    let other_values = bitmap(&[2, 3]);
    let other = SlotInput::new(DomainId::new(9), &input, SlotSet::borrowed(&other_values));
    let result = owned.subtract(other).unwrap();
    let SlotSet::Owned(result) = result.into_slots() else {
        panic!("owned subtraction must update its destination");
    };
    assert_eq!(result, bitmap(&[1, 4]));
}

#[test]
fn shared_result_owner_can_outlive_the_borrowed_input() {
    let owner = ResultOwner::new(DomainId::new(3), bitmap(&[5, 8]));
    let input = bitmap(&[5, 8, 13]);
    let borrowed = owner.share();
    let value = SlotInput::new(owner.domain(), &input, borrowed);
    assert_eq!(value.len(), 2);
    assert!(value.contains(8));
    assert_eq!(owner.len(), 2);
}
