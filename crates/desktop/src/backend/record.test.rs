use super::*;
use crate::backend::test_support::fixture;

#[test]
fn canonical_lookup_under_new_view_never_follows_a_retired_record_slot() {
    let fixture = fixture();
    let index = fixture.session.repository.index();
    let old_view = index.view().unwrap();
    let collection = fixture.session.collections[0].clone();
    let id = "path:a.json";
    let handle = old_view
        .file_handle_for_path(&fixture._root.path().join("a.json"))
        .unwrap();
    index.delete_file(handle).unwrap();
    let new_view = index.view().unwrap();
    assert!(matches!(
        record_by_id_on_view(&new_view, &collection, id, &CancellationToken::new()).unwrap(),
        DirectLookupResult::NotFound
    ));
    assert!(matches!(
        record_by_id_on_view(&old_view, &collection, id, &CancellationToken::new()).unwrap(),
        DirectLookupResult::Found(_)
    ));
}
