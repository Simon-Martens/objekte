use super::*;
use crate::model::LogicalType;
use crate::reference::{FieldDefinition, Schema};
use crate::relation::RelationDefinition;
use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions, SnapshotMode};
use crate::source::SourceFormat;


fn field(schema: &mut Schema, name: &str, kind: LogicalType, multi: bool) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: name == "id",
            nullable: name != "id",
            indexed: false,
            multi,
            element_nullable: vec![false],
        })
        .unwrap();
}

fn definition(name: &str, schema: Schema) -> CollectionDefinition {
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: Vec::new(),
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut entries = Schema::default();
    field(&mut entries, "id", LogicalType::Id, false);
    field(&mut entries, "mention", LogicalType::Id, false);
    entries
        .register_relation(RelationDefinition {
            path: vec!["mention".into()],
            target_collection: "mentions".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("entry_mentions".into()),
        })
        .unwrap();

    let mut mentions = Schema::default();
    field(&mut mentions, "id", LogicalType::Id, false);
    field(&mut mentions, "primary", LogicalType::Id, false);
    field(
        &mut mentions,
        "others",
        LogicalType::Array(Box::new(LogicalType::Id)),
        true,
    );
    for (path, cardinality, alias) in [
        ("primary", RelationCardinality::One, "primary_mentions"),
        ("others", RelationCardinality::Many, "other_mentions"),
    ] {
        mentions
            .register_relation(RelationDefinition {
                path: vec![path.into()],
                target_collection: "entries".into(),
                cardinality,
                reverse_alias: Some(alias.into()),
            })
            .unwrap();
    }

    let mut empty = Schema::default();
    field(&mut empty, "id", LogicalType::Id, false);
    vec![
        definition("entries", entries),
        definition("mentions", mentions),
        definition("empty", empty),
    ]
}

fn fixture() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-relation-inspection-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for name in ["entries", "mentions", "empty"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for (path, body) in [
        ("entries/a.json", r#"{"id":"A","mention":"m1"}"#),
        ("entries/b.json", r#"{"id":"B"}"#),
        (
            "mentions/m1.json",
            r#"{"id":"m1","primary":"A","others":["A","A","Z"]}"#,
        ),
        (
            "mentions/m2.json",
            r#"{"id":"m2","primary":null,"others":[]}"#,
        ),
        ("mentions/m3.json", r#"{"id":"m3"}"#),
        (
            "mentions/m4.json",
            r#"{"id":"m4","primary":"A","others":["A"]}"#,
        ),
        ("mentions/broken.json", "{broken"),
    ] {
        std::fs::write(root.join(path), body).unwrap();
    }
    root
}

fn inspect(
    repo: &Repository,
) -> (
    OutgoingReferencePage,
    OutgoingReferencePage,
    BacklinkPage,
    BacklinkPage,
    CollectionGraph,
) {
    let others = ["others".into()];
    let primary = ["primary".into()];
    let first = repo
        .relation_outgoing_page("mentions", &others, "m1", 0, 2)
        .unwrap();
    assert_eq!(first.presence, RelationPresence::Present);
    assert_eq!(first.total_occurrences, 3);
    assert_eq!(first.next_offset, Some(2));
    assert_eq!(first.references.len(), 2);
    assert_eq!(first.references[0].target_id, "A");
    assert_eq!(first.references[1].target_id, "A");
    assert_eq!(first.references[0].positions, [0]);
    assert_eq!(first.references[1].positions, [1]);
    assert!(first.references.iter().all(|edge| edge.resolved.is_some()));
    let last = repo
        .relation_outgoing_page("mentions", &others, "m1", 2, 2)
        .unwrap();
    assert_eq!(last.next_offset, None);
    assert_eq!(last.references.len(), 1);
    assert_eq!(last.references[0].target_id, "Z");
    assert_eq!(last.references[0].positions, [2]);
    assert!(last.references[0].resolved.is_none());
    assert_eq!(
        first,
        repo.relation_outgoing_page("mentions", &others, "m1", 0, 2)
            .unwrap()
    );

    let missing = repo
        .relation_outgoing_page("mentions", &primary, "m3", 0, 1)
        .unwrap();
    assert_eq!(missing.presence, RelationPresence::Missing);
    let null = repo
        .relation_outgoing_page("mentions", &primary, "m2", 0, 1)
        .unwrap();
    assert_eq!(null.presence, RelationPresence::Present);
    assert!(null.contains_null);
    assert_eq!(null.total_occurrences, 0);
    let empty = repo
        .relation_outgoing_page("mentions", &others, "m2", 0, 1)
        .unwrap();
    assert!(empty.contains_empty_array);
    assert_eq!(empty.total_occurrences, 0);
    let fallback_id = repo.collections()[1]
        .generation
        .records
        .iter()
        .find(|record| record.class == RecordClass::RawFallback)
        .unwrap()
        .id
        .0
        .clone();
    let fallback = repo
        .relation_outgoing_page("mentions", &others, &fallback_id, 0, 1)
        .unwrap();
    assert_eq!(fallback.presence, RelationPresence::Unavailable);
    assert_eq!(fallback.source.class, RecordClass::RawFallback);
    assert_eq!(fallback.source.file_status, FileStatus::Broken);
    assert_ne!(fallback.source.diagnostic_ref.0, 0);
    assert!(fallback.references.is_empty());

    let backlinks_first = repo
        .relation_backlink_page("entries", "other_mentions", "A", 0, 1)
        .unwrap();
    assert_eq!(backlinks_first.total_sources, 2);
    assert_eq!(backlinks_first.next_offset, Some(1));
    assert_eq!(backlinks_first.sources[0].id, "m1");
    let backlinks_second = repo
        .relation_backlink_page("entries", "other_mentions", "A", 1, 1)
        .unwrap();
    assert_eq!(backlinks_second.total_sources, 2);
    assert_eq!(backlinks_second.next_offset, None);
    assert_eq!(backlinks_second.sources[0].id, "m4");
    assert!(
        repo.relation_backlink_page("entries", "other_mentions", "A", 2, 1)
            .unwrap()
            .sources
            .is_empty()
    );

    let graph = repo.collection_graph().unwrap();
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.nodes[2].name, "empty");
    assert_eq!(graph.nodes[2].record_count, 0);
    assert!(graph.nodes[2].outgoing.is_empty());
    assert_eq!(graph.edges.len(), 3);
    assert_eq!(
        graph
            .edges
            .iter()
            .filter(|edge| edge.source == CollectionId(1) && edge.target == CollectionId(0))
            .count(),
        2
    );
    assert_eq!(
        graph.components[0].members,
        [CollectionId(0), CollectionId(1)]
    );
    assert!(graph.components[0].cyclic);
    assert_eq!(graph.components[1].members, [CollectionId(2)]);
    assert!(!graph.components[1].cyclic);
    assert!(graph.edges.iter().any(
        |edge| edge.reverse_alias.as_deref() == Some("other_mentions")
            && edge.occurrence_count == 4
            && edge.unresolved_occurrence_count == 1
    ));
    assert!(
        repo.relation_outgoing_page("mentions", &others, "m1", 0, 0)
            .is_err()
    );
    assert!(
        repo.relation_backlink_page(
            "entries",
            "other_mentions",
            "A",
            0,
            MAX_RELATION_PAGE_SIZE + 1
        )
        .is_err()
    );
    assert!(
        repo.relation_outgoing_page("mentions", &others, "unknown", 0, 1)
            .is_err()
    );
    assert!(
        repo.relation_backlink_page("entries", "other_mentions", "unknown", 0, 1)
            .is_err()
    );
    (first, last, backlinks_first, backlinks_second, graph)
}

#[test]
fn resident_inspection_paginates_and_restores_without_source_files() {
    let root = fixture();
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions(), options).unwrap();
    assert!(!built.startup().restored);
    let restored = Repository::open_with_options(&root, definitions(), options).unwrap();
    assert!(restored.startup().restored);
    for name in ["entries", "mentions", "empty"] {
        std::fs::remove_dir_all(root.join(name)).unwrap();
    }
    assert_eq!(inspect(&built), inspect(&restored));
    drop(restored);
    drop(built);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn collection_graph_marks_a_single_collection_self_cycle() {
    let edge = CollectionGraphEdge {
        id: RelationId {
            source: CollectionId(0),
            ordinal: 0,
        },
        source: CollectionId(0),
        path: vec!["self".into()],
        target: CollectionId(0),
        cardinality: RelationCardinality::One,
        reverse_alias: None,
        occurrence_count: 0,
        unresolved_occurrence_count: 0,
    };
    assert_eq!(
        graph_components(2, &[edge]),
        [
            CollectionComponent {
                members: vec![CollectionId(0)],
                cyclic: true,
            },
            CollectionComponent {
                members: vec![CollectionId(1)],
                cyclic: false,
            },
        ]
    );
}
