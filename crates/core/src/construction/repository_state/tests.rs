#[test]
fn fresh_fingerprints_require_every_shared_binding_to_agree() {
    use super::finish_fresh_fingerprints;
    use crate::inventory::{FileInventory, SourceRevision};
    use crate::model::CollectionId;

    let mut inventory = FileInventory::new(std::path::PathBuf::from("/tmp"));
    let revision = SourceRevision {
        modified_ns: 1,
        size: 3,
        object_id: None,
    };
    let file = inventory
        .add("shared.row".into(), revision, None, CollectionId(0))
        .unwrap();
    assert_eq!(
        inventory
            .add("shared.row".into(), revision, None, CollectionId(1))
            .unwrap(),
        file
    );
    let digest = [7; 32];
    assert_eq!(
        finish_fresh_fingerprints(
            &inventory,
            vec![vec![
                (CollectionId(0), digest),
                (CollectionId(0), digest),
                (CollectionId(1), digest),
            ]],
        )
        .unwrap(),
        [Some(digest)]
    );
    assert_eq!(
        finish_fresh_fingerprints(&inventory, vec![vec![(CollectionId(0), digest)]]).unwrap(),
        [None]
    );
    assert!(
        finish_fresh_fingerprints(
            &inventory,
            vec![vec![(CollectionId(0), digest), (CollectionId(1), [8; 32])]],
        )
        .is_err()
    );
}
