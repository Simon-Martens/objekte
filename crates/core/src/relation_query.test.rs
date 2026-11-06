use std::path::PathBuf;

use crate::model::LogicalType;
use crate::query::{CaseSensitivity, FtsMode, Query};
use crate::reference::{FieldDefinition, Schema};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use crate::source::SourceFormat;
use crate::workload::{ReturnMode, WorkloadCase};

fn field(schema: &mut Schema, name: &str, kind: LogicalType, multi: bool) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: false,
            nullable: true,
            indexed: false,
            multi,
            element_nullable: vec![false],
        })
        .unwrap();
}

fn relation(schema: &mut Schema, path: &str, target: &str, many: bool, alias: &str) {
    schema
        .register_relation(RelationDefinition {
            path: vec![path.into()],
            target_collection: target.into(),
            cardinality: if many {
                RelationCardinality::Many
            } else {
                RelationCardinality::One
            },
            reverse_alias: Some(alias.into()),
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
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn fixture() -> (PathBuf, Vec<CollectionDefinition>) {
    let root = std::env::temp_dir().join(format!(
        "objekte-relation-query-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    for name in ["entries", "mentions"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    for (name, text) in [
        (
            "entries/a.json",
            r#"{"id":"A","title":"Hymn","alt_title":"Hymn","related_entry":"B"}"#,
        ),
        (
            "entries/b.json",
            r#"{"id":"B","title":"Festival","alt_title":"Other","related_entry":"A"}"#,
        ),
        (
            "mentions/m1.json",
            r#"{"id":"m1","entry":"A","items":[{"entry":"A"},{"entry":"B"}],"title":"Festival","local_title":"Hymn"}"#,
        ),
        (
            "mentions/m2.json",
            r#"{"id":"m2","entry":"B","items":[{"entry":"B"}],"title":"Hymn","local_title":"x"}"#,
        ),
        (
            "mentions/m3.json",
            r#"{"id":"m3","entry":"Z","title":"Dangling"}"#,
        ),
        (
            "mentions/m4.json",
            r#"{"id":"m4","entry":null,"title":"Null"}"#,
        ),
        ("mentions/m5.json", r#"{"id":"m5","title":"Missing"}"#),
        (
            "mentions/m6.json",
            r#"{"id":"m6","contributors":["A","B"],"title":"Both"}"#,
        ),
        (
            "mentions/m7.json",
            r#"{"id":"m7","contributors":[],"title":"Empty"}"#,
        ),
        ("mentions/broken.json", "{broken"),
    ] {
        std::fs::write(root.join(name), text).unwrap();
    }
    let mut entries = Schema::default();
    field(&mut entries, "id", LogicalType::Id, false);
    field(&mut entries, "title", LogicalType::String, false);
    field(&mut entries, "alt_title", LogicalType::String, false);
    field(&mut entries, "related_entry", LogicalType::Id, false);
    relation(
        &mut entries,
        "related_entry",
        "entries",
        false,
        "related_from",
    );
    let mut mentions = Schema::default();
    field(&mut mentions, "id", LogicalType::Id, false);
    field(&mut mentions, "title", LogicalType::String, false);
    field(&mut mentions, "local_title", LogicalType::String, false);
    field(&mut mentions, "entry", LogicalType::Id, false);
    field(
        &mut mentions,
        "contributors",
        LogicalType::Array(Box::new(LogicalType::Id)),
        true,
    );
    mentions
        .register_field(FieldDefinition {
            path: vec!["items".into(), "entry".into()],
            kind: LogicalType::Id,
            required: false,
            nullable: true,
            indexed: false,
            multi: true,
            element_nullable: vec![false],
        })
        .unwrap();
    relation(&mut mentions, "entry", "entries", false, "mentions");
    relation(
        &mut mentions,
        "contributors",
        "entries",
        true,
        "contributions",
    );
    mentions
        .register_relation(RelationDefinition {
            path: vec!["items".into(), "entry".into()],
            target_collection: "entries".into(),
            cardinality: RelationCardinality::Many,
            reverse_alias: Some("item_mentions".into()),
        })
        .unwrap();
    (
        root,
        vec![
            definition("entries", entries),
            definition("mentions", mentions),
        ],
    )
}

fn query(
    collection: &str,
    filter: Option<&str>,
    fts: Option<&str>,
    field: Option<&str>,
    regex: bool,
) -> Query {
    Query::from_workload(&WorkloadCase {
        id: "relation".into(),
        collection: collection.into(),
        fts: fts.map(str::to_owned),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: if regex {
            FtsMode::Regex(Default::default())
        } else {
            FtsMode::Literal
        },
        filter: filter.map(str::to_owned),
        fts_fields: field.map(str::to_owned).into_iter().collect(),
        sort: vec![],
        limit: 50,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: true,
    })
    .unwrap()
}

fn ids(repo: &Repository, request: Query) -> Vec<String> {
    let optimized = repo.search(&request).unwrap();
    let reference = repo.search_reference(&request).unwrap();
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(optimized.count, reference.count);
    optimized
        .hits
        .iter()
        .map(|hit| hit.record_id.clone())
        .collect()
}

#[test]
fn finite_relation_paths_have_independent_reference_answers() {
    let (root, definitions) = fixture();
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, expected) in [
        ("entry.title = 'Hymn'", vec!["m1"]),
        ("entry.title:lower = 'hymn'", vec!["m1"]),
        ("entry.title ?!= 'Hymn'", vec!["m2"]),
        ("contributors.title = 'Hymn'", vec!["m7"]),
        ("contributors.title ?= 'Hymn'", vec!["m6"]),
        ("contributors[0].title = 'Hymn'", vec!["m6"]),
        ("contributors[1].title = 'Hymn'", vec![]),
        ("items[0].entry.title = 'Hymn'", vec!["m1"]),
        ("items[1].entry.title = 'Festival'", vec!["m1"]),
        ("local_title = entry.title", vec!["m1"]),
        ("entry.title = local_title", vec!["m1"]),
        ("entry.title = entry.alt_title", vec!["m1"]),
        ("contributors.title ?= contributors.alt_title", vec!["m6"]),
        ("entry.related_entry.title = 'Festival'", vec!["m1"]),
        (
            "entry.related_entry.related_entry.title = 'Hymn'",
            vec!["m1"],
        ),
    ] {
        assert_eq!(
            ids(&repo, query("mentions", Some(filter), None, None, false)),
            expected,
            "{filter}"
        );
    }
    assert_eq!(
        ids(
            &repo,
            query(
                "entries",
                Some("@backlinks.mentions.title ?~ 'Festival'"),
                None,
                None,
                false
            )
        ),
        ["A"],
    );
    assert_eq!(
        ids(
            &repo,
            query("mentions", None, Some("Hymn"), Some("entry.title"), false)
        ),
        ["m1"],
    );
    assert_eq!(
        ids(
            &repo,
            query("mentions", None, Some("^Hymn$"), Some("entry.title"), true)
        ),
        ["m1"],
    );
    assert_eq!(
        ids(
            &repo,
            query(
                "mentions",
                Some("title = 'Festival'"),
                Some("Hymn"),
                Some("entry.title"),
                false
            )
        ),
        ["m1"],
    );
    assert_eq!(
        ids(
            &repo,
            query(
                "entries",
                None,
                Some("Festival"),
                Some("@backlinks.mentions.title"),
                false
            )
        ),
        ["A"],
    );
    assert!(
        repo.search(&query(
            "entries",
            Some("@backlinks.unknown.title = 'x'"),
            None,
            None,
            false
        ))
        .is_err()
    );
    let mut sort = query("mentions", None, None, None, false);
    sort.options.sort.push(crate::workload::SortField {
        field: "entry.title".into(),
        descending: false,
    });
    assert!(
        repo.search(&sort)
            .unwrap_err()
            .contains("related sort field")
    );
}

#[test]
fn unavailable_targets_survive_negation_and_source_records_are_counted_once() {
    let (root, definitions) = fixture();
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, expected) in [
        (
            "!(entry.title = 'Hymn')",
            vec!["m2", "m4", "m5", "m6", "m7"],
        ),
        ("!(entry.title = 'Hymn') && @record.id = 'm3'", vec![]),
        ("@file.status = 1 && !(entry.title = 'Hymn')", vec![]),
        (
            "!(contributors.title ?= 'Hymn')",
            vec!["m1", "m2", "m3", "m4", "m5", "m7"],
        ),
        ("contributors.title ?!= 'Hymn'", vec!["m6"]),
    ] {
        assert_eq!(
            ids(&repo, query("mentions", Some(filter), None, None, false)),
            expected,
            "{filter}"
        );
    }
    let fallback_or = query(
        "mentions",
        Some("@file.status = 1 || entry.title = 'Hymn'"),
        None,
        None,
        false,
    );
    let optimized = repo.search(&fallback_or).unwrap();
    let reference = repo.search_reference(&fallback_or).unwrap();
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(optimized.count, Some(2));
    assert!(optimized.hits.iter().any(|hit| hit.record_id == "m1"));
    assert!(
        optimized
            .hits
            .iter()
            .any(|hit| hit.record_class == crate::inventory::RecordClass::RawFallback)
    );
    let selected = query(
        "mentions",
        Some("contributors.title ?= 'Hymn'"),
        None,
        None,
        false,
    );
    let found = repo.search(&selected).unwrap();
    assert_eq!(found.count, Some(1));
    assert_eq!(found.hits[0].record_id, "m6");
    assert!(
        found.hits[0]
            .matched_fields
            .contains(&"contributors.title".into())
    );
    assert!(found.counters.relation_edge_visits >= 1);
    assert!(found.counters.relation_target_reads >= 1);
    assert_eq!(found.counters.relation_source_reads, 0);
    let reference = repo.search_reference(&selected).unwrap();
    assert!(reference.counters.relation_source_reads > 0);
}

#[test]
fn relation_answers_and_evidence_survive_cache_and_snapshot_restore() {
    let (root, definitions) = fixture();
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let repo = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    repo.configure_verified_cache(1 << 20).unwrap();
    for filter in [
        "entry.title = 'Hymn'",
        "entry.title = 'Festival'",
        "!(entry.title = 'Hymn')",
        "entry.title = 'Hymn'",
    ] {
        let mut request = query("mentions", Some(filter), None, None, false);
        request.options.limit = 2;
        request.options.offset = 1;
        for _ in 0..2 {
            let actual = repo.search(&request).unwrap();
            let expected = repo.search_reference(&request).unwrap();
            assert_eq!(actual.count, expected.count, "{filter}");
            assert_eq!(actual.hits, expected.hits, "{filter}");
        }
    }
    for (mode, expected) in [
        (CaseSensitivity::Sensitive, 0),
        (CaseSensitivity::Insensitive, 1),
        (CaseSensitivity::Sensitive, 0),
    ] {
        let mut request = query("mentions", None, Some("HYMN"), Some("entry.title"), false);
        request.fts.as_mut().unwrap().case_sensitivity = mode;
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(actual.count, Some(expected));
        assert_eq!(actual.hits, reference.hits);
    }
    let restored = Repository::open_with_options(&root, definitions, options).unwrap();
    assert!(restored.startup().restored);
    for filter in ["entry.title = 'Hymn'", "contributors.title ?= 'Hymn'"] {
        let request = query("mentions", Some(filter), None, None, false);
        assert_eq!(
            repo.search(&request).unwrap().hits,
            restored.search(&request).unwrap().hits
        );
        assert_eq!(
            restored.search(&request).unwrap().hits,
            restored.search_reference(&request).unwrap().hits
        );
    }
}
