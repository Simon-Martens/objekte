use super::*;

#[test]
fn stale_compiled_model_cannot_replace_a_newer_persisted_manifest() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/minimal");
    let current = RepositoryModel::load(&root).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path(), &current.root, None, false).unwrap();
    store.reconcile_configuration(&current).unwrap();
    let path = current.files.keys().next().unwrap();
    let mut obsolete = current.clone();
    obsolete
        .files
        .insert(path.clone(), "obsolete-digest".into());
    assert!(matches!(
        store.reconcile_configuration(&obsolete),
        Err(Error::Unstable(_))
    ));
    let saved: String = store
        .db
        .query_row(
            "SELECT digest FROM inputs WHERE path=?1",
            [path.to_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(saved, current.files[path]);
    assert!(
        store
            .reconcile_configuration(&current)
            .unwrap()
            .inputs
            .is_empty()
    );
}
