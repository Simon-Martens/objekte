#[test]
fn parallel_metadata_preparation_keeps_order_revisions_and_overlapping_memberships() {
    use crate::construction::discovery::{prepare_discovered_file, prepare_discovered_window};
    use crate::inventory::FileInventory;
    use crate::model::CollectionId;
    use std::sync::atomic::AtomicUsize;

    let root = std::env::temp_dir().join(format!(
        "objekte-metadata-parallel-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let paths = (0..79)
        .map(|index| {
            let path = root.join(format!("{index:03}.json"));
            std::fs::write(&path, b"{\"name\":\"test\"}").unwrap();
            path
        })
        .collect::<Vec<_>>();
    let expected = paths
        .iter()
        .cloned()
        .map(|path| prepare_discovered_file(&root, path).unwrap())
        .collect::<Vec<_>>();
    for workers in [1, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        let peak = AtomicUsize::new(0);
        let prepared = prepare_discovered_window(&pool, &root, paths.clone(), true, &peak)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(prepared.len(), expected.len());
        for (actual, original) in prepared.iter().zip(&expected) {
            assert_eq!(actual.relative, original.relative);
            assert_eq!(actual.revision, original.revision);
            assert_eq!(actual.created_ns, original.created_ns);
        }
        let mut inventory = FileInventory::new(root.clone());
        for collection in [CollectionId(0), CollectionId(1)] {
            for entry in &prepared {
                inventory
                    .add(
                        entry.relative.clone(),
                        entry.revision,
                        entry.created_ns,
                        collection,
                    )
                    .unwrap();
            }
        }
        assert_eq!(inventory.files.len(), paths.len());
        for (index, file) in inventory.files.iter().enumerate() {
            assert_eq!(file.id.0 as usize, index);
            assert_eq!(file.collections, [CollectionId(0), CollectionId(1)]);
            assert_eq!(file.revision, expected[index].revision);
        }
        assert!(peak.load(std::sync::atomic::Ordering::Relaxed) >= 1);
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    let first_error = prepare_discovered_file(&root, std::env::temp_dir())
        .err()
        .unwrap();
    let results = prepare_discovered_window(
        &pool,
        &root,
        vec![std::env::temp_dir(), root.join("missing.json")],
        false,
        &AtomicUsize::new(0),
    );
    assert_eq!(
        results.into_iter().collect::<Result<Vec<_>, _>>().err(),
        Some(first_error)
    );
    std::fs::remove_dir_all(root).unwrap();
}
