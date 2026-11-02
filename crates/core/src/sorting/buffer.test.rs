use super::*;
use crate::{Direction, Sort};
use std::{cell::Cell, rc::Rc};

fn plans(direction: Direction) -> BTreeMap<String, SortPlan> {
    let sorts = [Sort {
        field: "rank".into(),
        direction,
    }];
    ["a", "b"]
        .into_iter()
        .map(|name| {
            (
                name.into(),
                SortPlan::compile_fields_with_language(&BTreeMap::new(), &sorts, None).unwrap(),
            )
        })
        .collect()
}

#[test]
fn heap_pages_match_full_sort_at_every_callback_checkpoint() {
    let input = [
        ("b", "z", "3"),
        ("a", "z", "7"),
        ("a", "b", "1.0"),
        ("a", "a", "1"),
        ("b", "a", "-4"),
        ("a", "m", "2"),
        ("a", "same", "1e2"),
        ("a", "same", "100.0"),
        ("b", "x", "9007199254740993"),
        ("a", "x", "-1"),
    ];
    for direction in [Direction::Asc, Direction::Desc] {
        for (offset, page_size, total_limit) in [
            (0, 0, 10),
            (0, 1, 10),
            (0, 4, 10),
            (2, 3, 10),
            (7, 2, 8),
            (usize::MAX, 2, 6),
        ] {
            let oracle_plans = plans(direction);
            let mut buffer = SortBuffer::new(plans(direction), offset, page_size, total_limit);
            let constructed = Cell::new(0);
            let mut observed = Vec::new();
            for (arrival, &(collection, id, number)) in input.iter().enumerate() {
                let keys = vec![SortValue::Number(number.into())];
                buffer.insert_keys(collection, id, keys.clone(), || {
                    constructed.set(constructed.get() + 1);
                    arrival
                });
                observed.push((collection, id, keys, arrival));
                let mut expected = observed.clone();
                expected.sort_by(|a, b| {
                    a.0.cmp(b.0)
                        .then_with(|| oracle_plans[a.0].compare(&a.2, a.1, &b.2, b.1))
                        .then_with(|| b.3.cmp(&a.3))
                });
                let keep = offset.saturating_add(page_size).min(total_limit);
                let expected = expected
                    .into_iter()
                    .take(keep)
                    .skip(offset)
                    .take(page_size)
                    .map(|item| item.3)
                    .collect::<Vec<_>>();
                assert_eq!(
                    buffer.page().copied().collect::<Vec<_>>(),
                    expected,
                    "direction={direction:?}, offset={offset}, arrival={arrival}"
                );
                buffer
                    .with_page(|page| {
                        assert_eq!(page, expected);
                        Ok(())
                    })
                    .unwrap();
            }
            assert!(constructed.get() <= input.len());
            assert_eq!(buffer.seen(), input.len());
            assert_eq!(buffer.total(true), Some(input.len().min(total_limit)));
            assert_eq!(buffer.truncated(), input.len() > total_limit);
            let expected_final = buffer.page().copied().collect::<Vec<_>>();
            assert_eq!(buffer.into_page(), expected_final);
        }
    }
}

#[test]
fn rejected_candidates_do_not_construct_payloads() {
    let mut buffer = SortBuffer::new(plans(Direction::Asc), 0, 1, 2);
    assert!(buffer.insert_keys("a", "first", vec![SortValue::Number("1".into())], || 1));
    assert!(
        !buffer.insert_keys("a", "last", vec![SortValue::Number("9".into())], || panic!(
            "rejected payload"
        ))
    );
    assert!(buffer.insert_keys("a", "better", vec![SortValue::Number("0".into())], || 0));
    assert_eq!(buffer.into_page(), [0]);
}

#[test]
fn callback_error_restores_owned_page_and_drops_each_payload_once() {
    struct Payload(Rc<Cell<usize>>);


    impl Drop for Payload {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }


    let dropped = Rc::new(Cell::new(0));
    let mut buffer = SortBuffer::new(plans(Direction::Asc), 0, 2, 10);
    for (id, rank) in [("b", "2"), ("a", "1")] {
        buffer.insert_keys("a", id, vec![SortValue::Number(rank.into())], || {
            Payload(dropped.clone())
        });
    }
    assert!(matches!(
        buffer.with_page(|page| {
            assert_eq!(page.len(), 2);
            Err(Error::Request("callback failed".into()))
        }),
        Err(Error::Request(_))
    ));
    assert_eq!(dropped.get(), 0);
    let page = buffer.into_page();
    assert_eq!(page.len(), 2);
    drop(page);
    assert_eq!(dropped.get(), 2);
}
