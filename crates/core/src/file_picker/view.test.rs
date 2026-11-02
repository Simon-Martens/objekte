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
