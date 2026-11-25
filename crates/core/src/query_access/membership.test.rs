use std::collections::BTreeSet;
use std::sync::Arc;

use roaring::RoaringBitmap;

use super::{DomainId, ResultOwner, SlotInput, SlotSet};

fn bitmap(values: &[u32]) -> RoaringBitmap {
    values.iter().copied().collect()
}

fn values(bitmap: &RoaringBitmap) -> BTreeSet<u32> {
    bitmap.iter().collect()
}

fn effective_values(input: &SlotInput<'_>) -> BTreeSet<u32> {
    input.iter().collect()
}

fn represented<'a>(
    representation: usize,
    domain: DomainId,
    input: &'a RoaringBitmap,
    members: &'a RoaringBitmap,
    shared: &'a RoaringBitmap,
) -> SlotInput<'a> {
    match representation {
        0 => SlotInput::new(domain, input, SlotSet::Empty),
        1 => SlotInput::unrestricted(domain, input),
        2 => SlotInput::new(domain, input, SlotSet::borrowed(members)),
        3 => SlotInput::new(domain, input, SlotSet::from_owned(members & input)),
        4 => SlotInput::new(
            domain,
            input,
            SlotSet::from_shared(Arc::new(shared & input)),
        ),
        5 => SlotInput::new(
            domain,
            input,
            SlotSet::from_sorted(Arc::from(members.iter().collect::<Vec<_>>())).unwrap(),
        ),
        _ => unreachable!(),
    }
}

fn oracle(representation: usize, input: &RoaringBitmap, members: &RoaringBitmap) -> BTreeSet<u32> {
    match representation {
        0 => BTreeSet::new(),
        1 => values(input),
        2..=5 => values(&(members & input)),
        _ => unreachable!(),
    }
}

#[test]
fn all_subset_representations_match_integer_set_oracle() {
    let domain = DomainId::from_parts(9, 2, 17, None);
    let full = bitmap(&[0, 1, 2, 3]);
    let narrow = bitmap(&[2, 3]);
    let backing = (0..16)
        .map(|mask| {
            (0..4)
                .filter(|slot| mask & (1 << slot) != 0)
                .collect::<RoaringBitmap>()
        })
        .collect::<Vec<_>>();
    let narrow_backing = backing
        .iter()
        .map(|members| members & &narrow)
        .collect::<Vec<_>>();
    let shared = (0..16)
        .map(|mask| {
            (0..4)
                .filter(|slot| mask & (1 << slot) != 0)
                .collect::<RoaringBitmap>()
        })
        .collect::<Vec<_>>();

    for left_mask in 0..16 {
        for right_mask in 0..16 {
            let left_input = if left_mask % 2 == 0 { &full } else { &narrow };
            let right_input = if right_mask % 3 == 0 { &narrow } else { &full };
            let left_members = if std::ptr::eq(left_input, &full) {
                &backing[left_mask]
            } else {
                &narrow_backing[left_mask]
            };
            let right_members = if std::ptr::eq(right_input, &full) {
                &backing[right_mask]
            } else {
                &narrow_backing[right_mask]
            };
            for left_rep in 0..6 {
                for right_rep in 0..6 {
                    let expected_left = oracle(left_rep, left_input, &backing[left_mask]);
                    let expected_right = oracle(right_rep, right_input, &backing[right_mask]);
                    let left = represented(
                        left_rep,
                        domain,
                        left_input,
                        left_members,
                        &shared[left_mask],
                    );
                    let right = represented(
                        right_rep,
                        domain,
                        right_input,
                        right_members,
                        &shared[right_mask],
                    );

                    assert_eq!(effective_values(&left), expected_left);
                    assert_eq!(effective_values(&right), expected_right);
                    assert_eq!(
                        left.same_members(&right).unwrap(),
                        expected_left == expected_right
                    );
                    assert_eq!(
                        left.is_subset(&right).unwrap(),
                        expected_left.is_subset(&expected_right)
                    );
                    assert_eq!(
                        left.is_disjoint(&right).unwrap(),
                        expected_left.is_disjoint(&expected_right)
                    );
                    assert_eq!(
                        left.intersection_len(&right).unwrap(),
                        expected_left.intersection(&expected_right).count() as u64
                    );
                    for slot in 0..5 {
                        assert_eq!(left.contains(slot), expected_left.contains(&slot));
                    }

                    let expected_union = expected_left
                        .union(&expected_right)
                        .copied()
                        .collect::<BTreeSet<_>>();
                    let union = represented(
                        left_rep,
                        domain,
                        left_input,
                        left_members,
                        &shared[left_mask],
                    )
                    .union(represented(
                        right_rep,
                        domain,
                        right_input,
                        right_members,
                        &shared[right_mask],
                    ))
                    .unwrap();
                    assert_eq!(
                        effective_values(&union),
                        expected_union,
                        "left_mask={left_mask} right_mask={right_mask} left_rep={left_rep} right_rep={right_rep} left_input={left_input:?} right_input={right_input:?}"
                    );

                    let expected_intersection = expected_left
                        .intersection(&expected_right)
                        .copied()
                        .collect::<BTreeSet<_>>();
                    let intersection = represented(
                        left_rep,
                        domain,
                        left_input,
                        left_members,
                        &shared[left_mask],
                    )
                    .intersect(represented(
                        right_rep,
                        domain,
                        right_input,
                        right_members,
                        &shared[right_mask],
                    ))
                    .unwrap();
                    assert_eq!(effective_values(&intersection), expected_intersection);

                    let expected_difference = expected_left
                        .difference(&expected_right)
                        .copied()
                        .collect::<BTreeSet<_>>();
                    let difference = represented(
                        left_rep,
                        domain,
                        left_input,
                        left_members,
                        &shared[left_mask],
                    )
                    .subtract(represented(
                        right_rep,
                        domain,
                        right_input,
                        right_members,
                        &shared[right_mask],
                    ))
                    .unwrap();
                    assert_eq!(
                        effective_values(&difference),
                        expected_difference,
                        "left_mask={left_mask} right_mask={right_mask} left_rep={left_rep} right_rep={right_rep} left_input={left_input:?} right_input={right_input:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn empty_has_one_meaning_across_representations() {
    let domain = DomainId::from_parts(1, 0, 1, None);
    let input = bitmap(&[1, 2, 3]);
    let empty = SlotInput::new(domain, &input, SlotSet::Empty);
    let borrowed_empty_values = RoaringBitmap::new();
    let borrowed_empty = SlotInput::new(domain, &input, SlotSet::borrowed(&borrowed_empty_values));
    let owned_empty = SlotInput::new(domain, &input, SlotSet::from_owned(RoaringBitmap::new()));
    let shared_empty = SlotInput::new(
        domain,
        &input,
        SlotSet::from_shared(Arc::new(RoaringBitmap::new())),
    );
    let all = SlotInput::unrestricted(domain, &input);
    for candidate in [&borrowed_empty, &owned_empty, &shared_empty] {
        assert!(empty.same_members(candidate).unwrap());
        assert!(empty.is_subset(&all).unwrap());
        assert!(empty.is_disjoint(&all).unwrap());
        assert_eq!(empty.intersection_len(&all).unwrap(), 0);
    }
    assert!(!empty.same_members(&all).unwrap());
}

#[test]
fn unrestricted_empty_backing_is_canonical_empty_for_every_representation() {
    let domain = DomainId::from_parts(1, 0, 1, None);
    let input = RoaringBitmap::new();
    let members = RoaringBitmap::new();
    let shared = RoaringBitmap::new();
    for left_rep in 0..6 {
        assert!(represented(left_rep, domain, &input, &members, &shared).is_empty());
        assert_eq!(
            represented(left_rep, domain, &input, &members, &shared).len(),
            0
        );
        assert_eq!(
            represented(left_rep, domain, &input, &members, &shared)
                .iter()
                .collect::<Vec<_>>(),
            Vec::<u32>::new()
        );
        for right_rep in 0..6 {
            assert!(
                represented(left_rep, domain, &input, &members, &shared)
                    .same_members(&represented(right_rep, domain, &input, &members, &shared))
                    .unwrap()
            );
            assert!(
                represented(left_rep, domain, &input, &members, &shared)
                    .is_subset(&represented(right_rep, domain, &input, &members, &shared))
                    .unwrap()
            );
            assert!(
                represented(right_rep, domain, &input, &members, &shared)
                    .is_subset(&represented(left_rep, domain, &input, &members, &shared))
                    .unwrap()
            );
            assert!(
                represented(left_rep, domain, &input, &members, &shared)
                    .is_disjoint(&represented(right_rep, domain, &input, &members, &shared))
                    .unwrap()
            );
            assert_eq!(
                represented(left_rep, domain, &input, &members, &shared)
                    .intersection_len(&represented(right_rep, domain, &input, &members, &shared))
                    .unwrap(),
                0
            );
            assert!(
                represented(left_rep, domain, &input, &members, &shared)
                    .intersect(represented(right_rep, domain, &input, &members, &shared))
                    .unwrap()
                    .is_empty()
            );
            assert!(
                represented(left_rep, domain, &input, &members, &shared)
                    .union(represented(right_rep, domain, &input, &members, &shared))
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[test]
fn every_domain_component_participates_in_identity() {
    let base = DomainId::from_parts(1, 2, 3, None);
    let values = bitmap(&[1, 2]);
    let input = values.clone();
    for different in [
        DomainId::from_parts(2, 2, 3, None),
        DomainId::from_parts(1, 3, 3, None),
        DomainId::from_parts(1, 2, 4, None),
        DomainId::from_parts(1, 2, 3, Some(0)),
        DomainId::from_parts(1, 2, 3, Some(1)),
    ] {
        let left = SlotInput::new(base, &input, SlotSet::borrowed(&values));
        let right = SlotInput::new(different, &input, SlotSet::borrowed(&values));
        assert!(left.same_members(&right).is_err());
        assert!(left.intersect(right).is_err());
    }

    let old_collision_a = DomainId::from_parts(0, 0, 0, Some(0));
    let old_collision_b = DomainId::from_parts(0, 0, 1, Some(131_072));
    assert_ne!(old_collision_a, old_collision_b);
}

#[test]
fn shared_owner_survives_borrowed_input_and_can_outlive_it() {
    let owner = ResultOwner::new(DomainId::new(3), bitmap(&[5, 8]));
    let borrowed_owner = owner.share();
    let input = bitmap(&[5, 8, 13]);
    let pointer = match &borrowed_owner {
        SlotSet::Shared(bitmap) => Arc::as_ptr(bitmap),
        _ => panic!("expected shared owner"),
    };
    let value = SlotInput::new(owner.domain(), &input, borrowed_owner);
    assert_eq!(value.len(), 2);
    assert!(value.contains(8));
    let SlotSet::Shared(shared) = value.into_slots() else {
        panic!("shared owner lost");
    };
    assert_eq!(Arc::as_ptr(&shared), pointer);
    assert_eq!(owner.len(), 2);
}

#[test]
fn sorted_membership_preserves_effective_input_and_rejects_invalid_slots() {
    use crate::query_access::contract::AccessError;

    let domain = DomainId::from_parts(9, 2, 17, None);
    let input = bitmap(&[1, 2, 3, 4]);
    let slots = SlotSet::from_sorted(Arc::from([1, 4])).unwrap();
    let narrowed = SlotInput::new(domain, &input, slots).replace_slots(SlotSet::Unrestricted);
    assert!(matches!(narrowed.slots(), SlotSet::Sorted(_)));
    assert_eq!(narrowed.bitmap_for_native().as_ref(), &bitmap(&[1, 4]));
    for slots in [[4, 1], [1, 1]] {
        assert!(matches!(
            SlotSet::from_sorted(Arc::from(slots)),
            Err(AccessError::Corrupt(_))
        ));
    }
}
