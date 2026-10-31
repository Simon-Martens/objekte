use objekte_core::Repository;
use std::path::PathBuf;


#[test]
fn musenalm_content_points_to_its_entry_without_an_embedded_copy() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut repo = Repository::open(&root).unwrap();
    let field = &repo.model().collections["contents"].fields["entry"];
    assert_eq!(field.relation.as_deref(), Some("entries"));
    assert!(field.index);

    let content = repo
        .get_record("contents", "8awao3ejwhyoz69")
        .unwrap()
        .unwrap();
    assert_eq!(content.data["entry"], "00fx8ln651na0e3");
    let edges = repo.references_in_record(&content).unwrap();
    assert!(edges.iter().any(|edge| {
        edge.location == "/entry"
            && edge.target_collection == "entries"
            && edge.target_id == "00fx8ln651na0e3"
    }));

    let entry = repo
        .get_record("entries", "00fx8ln651na0e3")
        .unwrap()
        .unwrap();
    assert!(entry.data.get("contents").is_none());
}
