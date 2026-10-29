mod support;
use objekte_core::{CacheMode, TargetState};
use serde_json::json;
use std::fs;
use support::*;
#[test]
fn outgoing_edges_preserve_occurrences_and_missing_targets() {
    for mode in [CacheMode::Disabled, CacheMode::Segments] {
        let f = Fixture::new();
        let mut r = f.repo(mode);
        let refs = r.references_from("books", "b", true).unwrap();
        assert_eq!(refs.edges.len(), 3);
        assert_eq!(
            refs.edges
                .iter()
                .filter(|e| e.target_state == TargetState::Present)
                .count(),
            2
        );
        assert_eq!(
            refs.edges
                .iter()
                .filter(|e| e.target_state == TargetState::Missing)
                .count(),
            1
        );
        assert_eq!(refs.edges[0].location, "/links/0/person");
        assert_eq!(refs.edges[1].location, "/links/1/person");
        assert!(r.metrics().source_reads >= 1);
        assert_eq!(refs.coverage[0].verified, mode == CacheMode::Segments);
    }
}

#[test]
fn segment_backlinks_scan_sources_on_demand() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    let refs = r.references_to("people", "p1", false, false).unwrap();
    assert_eq!(refs.edges.len(), 4);
    assert_eq!(refs.coverage[0].state, "current");
    assert!(refs.coverage[0].verified);
    r.get_record("books", "a").unwrap();
    let refs = r.references_to("people", "p1", false, false).unwrap();
    assert_eq!(refs.edges.len(), 4);
    assert_eq!(refs.coverage[0].state, "current");
    let refs = r.references_to("people", "p1", true, true).unwrap();
    assert_eq!(refs.edges.len(), 4);
    assert!(refs.coverage[0].verified);
    assert_eq!(refs.coverage[0].records, 7);
    assert!(
        r.cache_status()
            .unwrap()
            .collections
            .iter()
            .filter(|c| c.collection != "books")
            .all(|c| c.state == "absent")
    );
}

#[test]
fn backlink_changes_and_target_deletion_do_not_rewrite_referrers() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    r.open_view("books", &all()).unwrap();
    f.edit("data/books/a.json", |v| {
        v["person"] = json!("p2");
        v["links"] = json!([])
    });
    let m = r.refresh("books").unwrap();
    assert_eq!(m.rows_updated, 1);
    assert_eq!(m.edges_removed, 0);
    assert_eq!(m.edges_added, 0);
    assert_eq!(
        r.references_to("people", "p1", true, true)
            .unwrap()
            .edges
            .len(),
        2
    );
    assert_eq!(
        r.references_to("people", "p2", true, true)
            .unwrap()
            .edges
            .len(),
        1
    );
    fs::remove_file(f.root.join("data/people/p2.json")).unwrap();
    r.reset_metrics();
    let refs = r.references_to("people", "p2", true, true).unwrap();
    assert_eq!(refs.edges[0].target_state, TargetState::Missing);
    assert_eq!(r.metrics().rows_updated, 0);
    f.write("data/people/p2.json", &json!({"name":"Back"}));
    assert_eq!(
        r.references_to("people", "p2", true, true).unwrap().edges[0].target_state,
        TargetState::Present
    );
}

#[test]
fn missing_targets_later_appear_without_source_record_changes() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    let refs = r.references_from("books", "b", true).unwrap();
    assert!(
        refs.edges
            .iter()
            .any(|e| e.target_state == TargetState::Missing)
    );
    f.write("data/people/missing.json", &json!({"name":"Found"}));
    let refs = r.references_to("people", "missing", true, true).unwrap();
    assert_eq!(refs.edges.len(), 1);
    assert_eq!(refs.edges[0].target_state, TargetState::Present);
}

#[test]
fn deletion_removes_only_outgoing_edges_and_retains_incoming_edges() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    r.open_view("books", &all()).unwrap();
    r.refresh("people").unwrap();
    fs::remove_file(f.root.join("data/books/b.json")).unwrap();
    r.refresh("books").unwrap();
    assert_eq!(
        r.references_to("people", "p1", true, true)
            .unwrap()
            .edges
            .len(),
        2
    );
    let incoming = r.references_to("books", "b", true, true).unwrap();
    assert_eq!(incoming.edges.len(), 1);
    assert_eq!(incoming.edges[0].source_id, "p1");
    assert_eq!(incoming.edges[0].target_state, TargetState::Missing);
}

#[test]
fn renaming_a_source_id_changes_backlinks_without_an_edge_cache() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Segments);
    assert_eq!(
        repo.references_to("people", "p1", true, false)
            .unwrap()
            .edges
            .len(),
        4
    );
    fs::rename(
        f.root.join("data/books/a.json"),
        f.root.join("data/books/renamed.json"),
    )
    .unwrap();
    repo.refresh("books").unwrap();
    let edges = repo
        .references_to("people", "p1", true, false)
        .unwrap()
        .edges;
    assert_eq!(edges.len(), 4);
    assert!(edges.iter().any(|edge| edge.source_id == "renamed"));
    assert!(!edges.iter().any(|edge| edge.source_id == "a"));
}

#[test]
fn null_empty_and_missing_references_produce_no_edges() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Segments);
    for id in ["c", "d", "e"] {
        assert!(
            r.references_from("books", id, true)
                .unwrap()
                .edges
                .is_empty()
        )
    }
}

#[test]
fn direct_and_cached_backlinks_match_for_cycles() {
    let f = Fixture::new();
    for (target, id) in [
        ("books", "a"),
        ("books", "b"),
        ("people", "p1"),
        ("people", "missing"),
    ] {
        let a = f
            .repo(CacheMode::Disabled)
            .references_to(target, id, true, true)
            .unwrap();
        let b = f
            .repo(CacheMode::Segments)
            .references_to(target, id, true, true)
            .unwrap();
        let segments = f
            .repo(CacheMode::Segments)
            .references_to(target, id, true, true)
            .unwrap();
        assert_eq!(a.edges, b.edges);
        assert_eq!(a.edges, segments.edges);
        assert!(b.coverage.iter().all(|c| c.verified));
    }
}

#[test]
fn reopen_backlink_lookup_verifies_current_sources() {
    let f = Fixture::new();
    f.repo(CacheMode::Segments).refresh("books").unwrap();
    let mut r = f.repo(CacheMode::Segments);
    let partial = r.references_to("people", "p1", false, false).unwrap();
    assert_eq!(partial.coverage[0].state, "current");
    assert!(partial.coverage[0].verified);
    assert!(
        partial
            .edges
            .iter()
            .all(|e| e.target_state == TargetState::Unknown)
    );
    assert!(r.metrics().source_reads > 0);
    assert!(
        r.references_to("people", "p1", true, false)
            .unwrap()
            .coverage[0]
            .verified
    );
}

#[test]
fn concrete_reference_locations_escape_json_pointer_characters() {
    let f = Fixture::new();
    f.edit(".editor/books/schema.json", |v| {
        v["properties"]["a/b~c"] = json!({"type":"string","x-relation":{"collection":"people"}})
    });
    f.edit("data/books/a.json", |v| v["a/b~c"] = json!("p1"));
    let refs = f
        .repo(CacheMode::Segments)
        .references_from("books", "a", false)
        .unwrap();
    assert!(refs.edges.iter().any(|e| e.location == "/a~1b~0c"));
}
