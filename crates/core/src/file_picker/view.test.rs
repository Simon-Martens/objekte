use super::*;

#[test]
fn borrowed_view_columns_encode_like_legacy_owned_columns() {
    let mut index = ViewIndex::new(&["rank".into(), "title".into()], &BTreeMap::new());
    index.keys = vec![
        vec![Scalar::Int(3), Scalar::Null],
        vec![Scalar::String("Äpfel".into()), Scalar::String("a".into())],
    ];
    index.ordered = vec![vec![0, 1], vec![1, 0]];
    // bincode serializes the old owned struct as its three fields in order.
    let legacy = bincode::serialize(&(&index.fields, &index.keys, &index.ordered)).unwrap();
    let borrowed = bincode::serialize(&index.saved()).unwrap();
    assert_eq!(borrowed, legacy);
    let restored: RestoredViewIndex = bincode::deserialize(&legacy).unwrap();
    assert_eq!(restored.fields, index.fields);
    assert_eq!(restored.ordered, index.ordered);
    assert_eq!(restored.keys.len(), index.keys.len());
}

#[test]
fn parallel_field_order_matches_independent_serial_sort() {
    let count = 9_000;
    let index = ViewIndex::new(&["a".into(), "b".into(), "c".into()], &BTreeMap::new());
    let mut ids = Vec::new();
    let files = (0..count)
        .map(|slot| {
            let start = ids.len();
            ids.extend_from_slice(format!("{slot:05}").as_bytes());
            Some(FileEntry {
                id_start: start as u32,
                id_len: 5,
                stamp: crate::source::Stamp {
                    size: 0,
                    modified: String::new(),
                    identity: String::new(),
                },
                revision: 0,
                changed_at_nanos: 0,
            })
        })
        .collect::<Vec<_>>();
    let columns = vec![
        (0..count)
            .map(|slot| match slot % 5 {
                0 => Scalar::Missing,
                1 => Scalar::Null,
                2 => Scalar::Invalid,
                _ => Scalar::Int((slot % 13) as i64),
            })
            .collect::<Vec<_>>(),
        (0..count)
            .map(|slot| Scalar::String(format!("{:02}", slot % 17)))
            .collect::<Vec<_>>(),
        (0..count)
            .map(|slot| Scalar::Int((count - slot) as i64))
            .collect::<Vec<_>>(),
    ];
    let parallel = index.ordered_columns(&columns, &files, &ids);
    for (field, order) in parallel.iter().enumerate() {
        let mut serial = (0..count).collect::<Vec<_>>();
        serial.sort_by(|&a, &b| compare_slots(&columns[field], a, b, &files, &ids));
        assert_eq!(*order, serial, "field {field}");
    }
}
