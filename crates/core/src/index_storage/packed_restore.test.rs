use crate::inference::{InferenceConfig, InferenceMode};
use crate::lifecycle::Index;
use crate::model::{FieldPath, LogicalType};
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::source::{SourceFormat, SourceRegistry};
use crate::workload::{ReturnMode, SortField};

use super::store::SaveOptions;

fn field(path: &str, kind: LogicalType) -> FieldDefinition {
    FieldDefinition {
        path: vec![path.into()],
        kind,
        required: false,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: vec![],
    }
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut entries = Schema::default();
    entries
        .register_field(field("id", LogicalType::Id))
        .unwrap();
    entries
        .register_field(field("title", LogicalType::String))
        .unwrap();
    entries
        .register_field(field("text", LogicalType::String))
        .unwrap();
    let mut mentions = Schema::default();
    mentions
        .register_field(field("id", LogicalType::Id))
        .unwrap();
    mentions
        .register_field(field("title", LogicalType::String))
        .unwrap();
    mentions
        .register_field(field("parent", LogicalType::Id))
        .unwrap();
    mentions
        .register_relation(RelationDefinition {
            path: vec!["parent".into()],
            target_collection: "entries".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("mentions".into()),
        })
        .unwrap();
    [
        ("entries", entries, "entries"),
        ("mentions", mentions, "mentions"),
    ]
    .into_iter()
    .map(|(name, schema, source)| CollectionDefinition {
        name: name.into(),
        source_path: source.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("title").unwrap()],
        sort_locale: "und".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    })
    .collect()
}

fn fixture() -> (tempfile::TempDir, Vec<CollectionDefinition>) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("entries")).unwrap();
    std::fs::create_dir(root.path().join("mentions")).unwrap();
    std::fs::write(
        root.path().join("entries/a.json"),
        br#"{"id":"a","title":"Alpha","text":"Alpha story"}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("entries/b.json"),
        br#"{"id":"b","title":"Beta","text":"Beta story"}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("mentions/m.json"),
        br#"{"id":"m","title":"Mention","parent":"a"}"#,
    )
    .unwrap();
    (root, definitions())
}

fn query(collection: &str, filter: Option<&str>, fts: Option<&str>, sort: Vec<SortField>) -> Query {
    Query {
        collections: vec![collection.into()],
        fts: fts.map(|literal| FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: literal.into(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        filter: filter.map(|filter| parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort,
            limit: 50,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

fn ordered_hits(index: &Index, query: &Query) -> Vec<(String, String)> {
    index
        .view()
        .unwrap()
        .search(query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| (hit.collection, hit.record_id))
        .collect()
}

fn hits(index: &Index, query: &Query) -> Vec<(String, String)> {
    let mut hits = index
        .view()
        .unwrap()
        .search(query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| (hit.collection, hit.record_id))
        .collect::<Vec<_>>();
    hits.sort();
    hits
}

#[test]
fn packed_restore_preserves_auto_index_inference() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(
        root.path().join("data/a.json"),
        br#"{"title":"Alpha","year":2024}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("data/b.json"),
        br#"{"title":"Beta","year":2023}"#,
    )
    .unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "files",
        "data",
        SourceFormat::Json,
    )];
    let options = RepositoryOptions {
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        ..Default::default()
    };
    let index = Index::build(root.path(), definitions.clone(), options).unwrap();
    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        store.path(),
        definitions,
        options,
        SourceRegistry::builtin(),
    )
    .unwrap();
    for filter in ["year = 2024", "year >= 2024", "title = \"Alpha\""] {
        let request = query("files", Some(filter), None, Vec::new());
        let built = hits(&index, &request);
        assert!(!built.is_empty(), "{filter}");
        assert_eq!(built, hits(&loaded, &request), "{filter}");
    }
}

#[test]
fn packed_restore_matches_typed_fts_sort_and_relation_queries() {
    let (root, definitions) = fixture();
    let index = Index::build(
        root.path(),
        definitions.clone(),
        RepositoryOptions::default(),
    )
    .unwrap();
    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        store.path(),
        definitions,
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();

    let loaded_view = loaded.view().unwrap();
    let loaded_segment = loaded_view.0.segments[0].first().expect("loaded segment");
    let loaded_column = loaded_segment
        .index
        .sorting
        .column("title")
        .expect("loaded sort column");
    assert!(loaded_column.keys.is_empty());
    assert!(loaded_column.ascending.is_empty());
    assert!(loaded_column.descending.is_empty());
    assert!(loaded_column.packed.is_some());
    let loaded_relation = loaded_view.0.segments[1]
        .first()
        .expect("loaded mentions segment")
        .index
        .relations
        .first()
        .expect("loaded relation contribution");
    assert!(loaded_relation.packed.is_some());
    assert!(loaded_relation.references.is_empty());
    assert!(loaded_relation.present.is_empty());
    assert!(loaded_relation.nulls.is_empty());
    drop(loaded_view);

    let queries = [
        query("entries", Some("title = \"Alpha\""), None, Vec::new()),
        query("entries", Some("title != \"Beta\""), None, Vec::new()),
        query("entries", None, Some("Alpha"), Vec::new()),
        query(
            "entries",
            None,
            None,
            vec![SortField {
                field: "title".into(),
                descending: false,
                expression: None,
            }],
        ),
        query(
            "mentions",
            Some("parent.title = \"Alpha\""),
            None,
            Vec::new(),
        ),
        query(
            "mentions",
            Some("parent.title = \"Alpha revised\""),
            None,
            Vec::new(),
        ),
    ];
    for (position, request) in queries.into_iter().enumerate() {
        let built = hits(&index, &request);
        assert_eq!(built, hits(&loaded, &request), "{request:?}");
        if position == 3 {
            assert_eq!(
                ordered_hits(&index, &request),
                ordered_hits(&loaded, &request),
                "sorted order"
            );
        }
        if matches!(position, 0 | 2 | 4) {
            assert!(
                !built.is_empty(),
                "query family {position} unexpectedly empty"
            );
        }
    }
    let relation_catalog = loaded.view().unwrap();
    let mentions = &relation_catalog.repository().collections()[1];
    assert_eq!(mentions.definition.name, "mentions");
}

#[test]
fn packed_restore_serves_posting_values_without_native_dictionaries() {
    let (root, definitions) = fixture();
    let index = Index::build(
        root.path(),
        definitions.clone(),
        RepositoryOptions::default(),
    )
    .unwrap();
    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        store.path(),
        definitions,
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let view = loaded.view().unwrap();
    let collection = &view.repository().collections()[0];
    assert!(
        collection
            .postings
            .fields
            .values()
            .all(|field| field.values.is_empty())
    );
    assert!(collection.raw.normal.is_empty());
    assert!(collection.generation.records.is_empty());
    assert!(collection.id_lookup.is_empty());
    let segment = view.0.segments[0].first().expect("segment");
    assert!(segment.index.postings.fields.is_empty());
    assert!(segment.index.raw.normal.is_empty());
    assert!(segment.index.records.as_slice().is_none());
    assert_eq!(segment.index.records.materialized_rows(), 0);
    assert!(segment.index.id_lookup.native().is_none());
    assert_eq!(
        segment.index.records.len(),
        segment.index.identity_order.len()
    );

    let request = crate::posting_values::PostingValuesRequest::new("entries", "title", 100);
    let built = index.view().unwrap().posting_values(&request).unwrap();
    let packed = view.posting_values(&request).unwrap();
    assert_eq!(built, packed);
    assert_eq!(packed.coverage.present_records, 2);
    assert_eq!(packed.coverage.explicit_null_records, 0);
    assert!(packed.values.iter().any(|entry| {
        entry.value == crate::posting::ScalarKey::String("Alpha".into()) && entry.count == 1
    }));
}

#[test]
fn loaded_packed_compaction_preserves_typed_fts_sort_and_relations() {
    let (root, definitions) = fixture();
    let index = Index::build(
        root.path(),
        definitions.clone(),
        RepositoryOptions::default(),
    )
    .unwrap();
    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        store.path(),
        definitions.clone(),
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let pinned = loaded.view().unwrap();
    let old_alpha = query("entries", Some("title = \"Alpha\""), None, Vec::new());
    let old_relation = query(
        "mentions",
        Some("parent.title = \"Alpha\""),
        None,
        Vec::new(),
    );
    assert_eq!(pinned.search(&old_alpha).unwrap().count, Some(1));
    assert_eq!(pinned.search(&old_relation).unwrap().count, Some(1));

    // Replace one entry, add another, and delete the only mention.
    std::fs::write(
        root.path().join("entries/a.json"),
        br#"{"id":"a","title":"Alpha revised","text":"Alpha revised story"}"#,
    )
    .unwrap();
    let handle = pinned
        .file_handle_for_path(&root.path().join("entries/a.json"))
        .expect("entry file handle");
    loaded
        .update_file(handle, &root.path().join("entries/a.json"))
        .unwrap();
    let mention_handle = pinned
        .file_handle_for_path(&root.path().join("mentions/m.json"))
        .expect("mention file handle");
    let mention_file = mention_handle.file_id;
    loaded.delete_file(mention_handle).unwrap();
    std::fs::remove_file(root.path().join("mentions/m.json")).unwrap();
    std::fs::write(
        root.path().join("entries/c.json"),
        br#"{"id":"c","title":"Gamma","text":"Gamma story"}"#,
    )
    .unwrap();
    loaded
        .add_file(&root.path().join("entries/c.json"))
        .unwrap();
    std::fs::write(
        root.path().join("mentions/n.json"),
        br#"{"id":"n","title":"New mention","parent":"a"}"#,
    )
    .unwrap();
    loaded
        .add_file(&root.path().join("mentions/n.json"))
        .unwrap();

    let oracle = Index::build(
        root.path(),
        definitions.clone(),
        RepositoryOptions::default(),
    )
    .unwrap();
    let requests = [
        query(
            "entries",
            Some("title = \"Alpha revised\""),
            None,
            Vec::new(),
        ),
        query("entries", Some("title = \"Alpha\""), None, Vec::new()),
        query("entries", None, Some("Gamma"), Vec::new()),
        query(
            "entries",
            None,
            None,
            vec![SortField {
                field: "title".into(),
                descending: false,
                expression: None,
            }],
        ),
        query(
            "entries",
            None,
            None,
            vec![SortField {
                field: "title".into(),
                descending: true,
                expression: None,
            }],
        ),
        query(
            "mentions",
            Some("parent.title = \"Alpha revised\""),
            None,
            Vec::new(),
        ),
    ];
    // Source-free maintenance: every expected result was captured before the
    // source tree disappears.
    std::fs::remove_dir_all(root.path().join("entries")).unwrap();
    std::fs::remove_dir_all(root.path().join("mentions")).unwrap();
    let report = loaded.compact().unwrap();
    assert!(report.retired_slots_removed > 0);
    let compacted_view = loaded.view().unwrap();
    let packed_parts = compacted_view.0.segments[0]
        .iter()
        .filter(|segment| segment.packed.is_some())
        .collect::<Vec<_>>();
    assert!(!packed_parts.is_empty());
    assert!(
        packed_parts
            .iter()
            .all(|segment| segment.index.records.as_slice().is_none())
    );
    assert!(
        packed_parts
            .iter()
            .all(|segment| segment.index.records.materialized_rows() == 0)
    );
    for segment in &packed_parts {
        assert!(
            segment
                .index
                .postings
                .fields
                .values()
                .all(|field| field.values.is_empty())
        );
        assert!(segment.index.raw.normal.is_empty());
        for column in segment.index.sorting.columns.values() {
            assert!(column.packed.is_some());
            assert!(column.keys.is_empty());
            assert!(column.ascending.is_empty());
            assert!(column.descending.is_empty());
        }
        for relation in &segment.index.relations {
            assert!(relation.packed.is_some());
            assert!(relation.references.is_empty());
            assert!(relation.present.is_empty());
        }
    }
    assert!(
        compacted_view
            .0
            .catalog
            .inactive_entry(mention_file)
            .is_some()
    );
    drop(compacted_view);
    for request in &requests {
        assert_eq!(
            hits(&oracle, request),
            hits(&loaded, request),
            "{request:?}"
        );
    }
    // The pinned pre-mutation view still answers from its old publication.
    assert_eq!(pinned.search(&old_alpha).unwrap().count, Some(1));

    let compacted = tempfile::tempdir().unwrap();
    loaded
        .save_to(compacted.path(), SaveOptions::default())
        .unwrap();
    let reloaded = Index::load_from(
        compacted.path(),
        definitions,
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let view = reloaded.view().unwrap();
    let segment = view.0.segments[0]
        .iter()
        .find(|segment| segment.ordinal > 0)
        .expect("compacted packed segment");
    assert!(segment.packed.is_some());
    assert!(segment.index.records.as_slice().is_none());
    assert_eq!(segment.index.records.materialized_rows(), 0);
    assert!(segment.index.id_lookup.native().is_none());
    drop(view);
    for request in &requests {
        assert_eq!(
            hits(&oracle, request),
            hits(&reloaded, request),
            "reloaded {request:?}"
        );
    }
}
