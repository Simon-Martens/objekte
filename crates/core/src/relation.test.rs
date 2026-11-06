use super::*;
use crate::reference::FieldDefinition;
use crate::repository::{IdentityPolicy, Repository, RepositoryOptions, SnapshotMode};
use crate::source::SourceFormat;

fn field(schema: &mut Schema, path: &[&str], kind: LogicalType, multi: bool, nullable: bool) {
    schema
        .register_field(FieldDefinition {
            path: path.iter().map(|part| (*part).to_owned()).collect(),
            kind,
            required: false,
            nullable,
            indexed: false,
            multi,
            element_nullable: vec![false],
        })
        .unwrap();
}

fn relation(
    path: &[&str],
    target: &str,
    cardinality: RelationCardinality,
    alias: &str,
) -> RelationDefinition {
    RelationDefinition {
        path: path.iter().map(|part| (*part).to_owned()).collect(),
        target_collection: target.into(),
        cardinality,
        reverse_alias: Some(alias.into()),
    }
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

fn definitions(entry_target: &str) -> Vec<CollectionDefinition> {
    let mut targets = Schema::default();
    field(&mut targets, &["id"], LogicalType::Id, false, false);
    let entries = definition("entries", targets.clone());
    let people = definition("people", targets);
    let mut mentions = Schema::default();
    field(&mut mentions, &["id"], LogicalType::Id, false, false);
    field(&mut mentions, &["entry"], LogicalType::Id, false, true);
    field(
        &mut mentions,
        &["contributors"],
        LogicalType::Array(Box::new(LogicalType::Id)),
        true,
        true,
    );
    field(
        &mut mentions,
        &["items", "entry"],
        LogicalType::Id,
        true,
        false,
    );
    field(
        &mut mentions,
        &["groups", "items", "entry"],
        LogicalType::Id,
        true,
        false,
    );
    mentions
        .register_relation(relation(
            &["entry"],
            entry_target,
            RelationCardinality::One,
            "mentions",
        ))
        .unwrap();
    mentions
        .register_relation(relation(
            &["contributors"],
            "people",
            RelationCardinality::Many,
            "contributions",
        ))
        .unwrap();
    mentions
        .register_relation(relation(
            &["items", "entry"],
            "entries",
            RelationCardinality::Many,
            "item_mentions",
        ))
        .unwrap();
    mentions
        .register_relation(relation(
            &["groups", "items", "entry"],
            "entries",
            RelationCardinality::Many,
            "group_mentions",
        ))
        .unwrap();
    assert!(mentions.fields()[&vec!["entry".into()]].indexed);
    vec![entries, people, definition("mentions", mentions)]
}

fn fixture(name: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-relation-index-{name}-{}",
        std::process::id()
    ));
    for collection in ["entries", "people", "mentions"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
    }
    for (path, bytes) in [
        ("entries/a.json", r#"{"id":"A"}"#),
        ("entries/b.json", r#"{"id":"B"}"#),
        ("people/a.json", r#"{"id":"A"}"#),
        ("people/p.json", r#"{"id":"P"}"#),
        (
            "mentions/m1.json",
            r#"{"id":"m1","entry":"A","contributors":["P","A","P"],"items":[{"entry":"A"},{"entry":"Z"}],"groups":[{"items":[{"entry":"A"},{"entry":"A"}]},{"items":[{"entry":"B"}]}]}"#,
        ),
        (
            "mentions/m2.json",
            r#"{"id":"m2","entry":"Z","contributors":[],"items":[],"groups":[{"items":[]}]}"#,
        ),
        (
            "mentions/m3.json",
            r#"{"id":"m3","entry":null,"contributors":null}"#,
        ),
        ("mentions/m4.json", r#"{"id":"m4"}"#),
        ("mentions/broken.json", "{broken"),
    ] {
        std::fs::write(root.join(path), bytes).unwrap();
    }
    root
}

fn check(repo: &Repository) {
    let entries = &repo.collections()[0];
    let people = &repo.collections()[1];
    let mentions = &repo.collections()[2];
    let m1 = mentions.id_lookup["m1"];
    let m2 = mentions.id_lookup["m2"];
    let m3 = mentions.id_lookup["m3"];
    let m4 = mentions.id_lookup["m4"];
    let fallback = mentions
        .generation
        .records
        .iter()
        .find(|record| record.class == RecordClass::RawFallback)
        .unwrap()
        .slot;
    assert!(entries.relations.is_empty());
    assert!(people.relations.is_empty());
    assert_eq!(mentions.relations.len(), 4);

    let entry = repo.relation_index("mentions", &["entry".into()]).unwrap();
    assert_eq!(entry.id.source, mentions.generation.collection);
    assert_eq!(entry.target_collection, entries.generation.collection);
    assert_eq!(entry.edges(m1).unwrap().len(), 1);
    assert_eq!(
        entry.edges(m1).unwrap()[0].target,
        RelationTarget::Resolved(entries.id_lookup["A"])
    );
    assert_eq!(
        entry.edges(m2).unwrap()[0].target,
        RelationTarget::Unresolved("Z".into())
    );
    assert!(entry.edges(m3).unwrap().is_empty());
    assert!(entry.edges(m4).unwrap().is_empty());
    assert!(entry.present().contains(m3.0));
    assert!(entry.nulls().contains(m3.0));
    assert!(!entry.present().contains(m4.0));
    assert!(entry.unavailable().contains(fallback.0));
    assert!(!entry.present().contains(fallback.0));
    assert_eq!(entry.unresolved_count(), 1);
    assert_eq!(
        entry
            .unresolved_references()
            .map(|(slot, id, positions)| (slot, id.to_owned(), positions.to_vec()))
            .collect::<Vec<_>>(),
        [(m2, "Z".into(), vec![])]
    );
    assert_eq!(
        mentions.generation.records[m2.0 as usize].class,
        RecordClass::Logical
    );
    assert_eq!(
        repo.relation_reverse_sources(entry, "A")
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [m1.0]
    );
    assert!(std::ptr::eq(
        repo.backlink_relation("entries", "mentions").unwrap(),
        entry
    ));

    let contributors = repo
        .relation_index("mentions", &["contributors".into()])
        .unwrap();
    assert_eq!(contributors.target_collection, people.generation.collection);
    let links = contributors.edges(m1).unwrap();
    assert_eq!(links.len(), 3);
    assert_eq!(
        links
            .iter()
            .map(|edge| edge.positions.as_slice())
            .collect::<Vec<_>>(),
        [&[0][..], &[1][..], &[2][..]]
    );
    assert_eq!(
        links[0].target,
        RelationTarget::Resolved(people.id_lookup["P"])
    );
    assert_eq!(
        links[1].target,
        RelationTarget::Resolved(people.id_lookup["A"])
    );
    assert_eq!(
        repo.relation_reverse_sources(contributors, "P")
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [m1.0]
    );
    assert!(contributors.present().contains(m2.0));
    assert!(contributors.empty_arrays().contains(m2.0));
    assert!(contributors.nulls().contains(m3.0));

    let nested = repo
        .relation_index("mentions", &["items".into(), "entry".into()])
        .unwrap();
    assert_eq!(
        nested
            .edges(m1)
            .unwrap()
            .iter()
            .map(|edge| edge.positions.as_slice())
            .collect::<Vec<_>>(),
        [&[0][..], &[1][..]]
    );
    assert_eq!(
        nested.edges(m1).unwrap()[1].target,
        RelationTarget::Unresolved("Z".into())
    );
    assert_eq!(nested.unresolved_count(), 1);
    assert!(nested.empty_arrays().contains(m2.0));
    assert!(nested.present().contains(m2.0));
    assert!(nested.estimated_heap_bytes() > 0);
    assert!(nested.estimated_heap_bytes() < 4096);
    let groups = repo
        .relation_index(
            "mentions",
            &["groups".into(), "items".into(), "entry".into()],
        )
        .unwrap();
    assert_eq!(
        groups
            .edges(m1)
            .unwrap()
            .iter()
            .map(|edge| edge.positions.as_slice())
            .collect::<Vec<_>>(),
        [&[0, 0][..], &[0, 1][..], &[1, 0][..]]
    );
    assert_eq!(groups.edges(m2).unwrap(), []);
    assert!(groups.empty_arrays().contains(m2.0));
    assert!(groups.present().contains(m2.0));
    assert_eq!(
        repo.relation_reverse_sources(groups, "A")
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [m1.0]
    );
    let file = &repo.files().files[mentions.generation.records[m1.0 as usize].file.0 as usize];
    assert!(file.relative_path.ends_with("mentions/m1.json"));
    validate_indexes(repo.collections()).unwrap();
    let mut corrupted = nested.clone();
    corrupted.edges[0].target = RelationTarget::Resolved(people.id_lookup["P"]);
    assert!(
        corrupted
            .validate(mentions, entries)
            .unwrap_err()
            .contains("forward/reverse")
    );
}

#[test]
fn nested_array_occurrences_retain_every_scope_position() {
    use crate::model::ArrayValue;

    let item = |id: &str| Value::Object(BTreeMap::from([("entry".into(), Value::Id(id.into()))]));
    let items = |ids: &[&str]| {
        Value::Array(
            ArrayValue::new(
                ids.iter().map(|id| item(id)).collect(),
                Some(LogicalType::Object),
                false,
            )
            .unwrap(),
        )
    };
    let group = |ids: &[&str]| Value::Object(BTreeMap::from([("items".into(), items(ids))]));
    let value = Value::Object(BTreeMap::from([(
        "groups".into(),
        Value::Array(
            ArrayValue::new(
                vec![group(&["A", "A"]), group(&["B"])],
                Some(LogicalType::Object),
                false,
            )
            .unwrap(),
        ),
    )]));
    let mut contribution = RelationContribution {
        declaration: relation(
            &["groups", "items", "entry"],
            "entries",
            RelationCardinality::Many,
            "group_mentions",
        ),
        record_count: 0,
        references: Vec::new(),
        present: RoaringBitmap::new(),
        nulls: RoaringBitmap::new(),
        empty_arrays: RoaringBitmap::new(),
        unavailable: RoaringBitmap::new(),
    };
    contribution.add_logical(RecordSlot(0), &value).unwrap();
    assert_eq!(
        contribution
            .references
            .iter()
            .map(|reference| (reference.target_id.as_str(), reference.positions.as_slice()))
            .collect::<Vec<_>>(),
        [("A", &[0, 0][..]), ("A", &[0, 1][..]), ("B", &[1, 0][..])]
    );
}

#[test]
fn declared_links_resolve_once_and_survive_snapshot_restore() {
    let root = fixture("restore");
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions("entries"), options).unwrap();
    assert!(!built.startup().restored);
    assert_eq!(built.build_parses(), 8);
    assert_eq!(built.build_failed_parses(), 1);
    check(&built);
    let restored = Repository::open_with_options(&root, definitions("entries"), options).unwrap();
    assert!(restored.startup().restored);
    assert_eq!(restored.build_parses(), 0);
    check(&restored);
    assert_eq!(
        format!("{:?}", built.collections()[2].relations),
        format!("{:?}", restored.collections()[2].relations)
    );
    let changed = Repository::open_with_options(&root, definitions("people"), options).unwrap();
    assert!(!changed.startup().restored);
    assert_eq!(
        changed
            .relation_index("mentions", &["entry".into()])
            .unwrap()
            .target_collection,
        changed.collections()[1].generation.collection
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relation_configuration_rejects_wrong_types_targets_and_alias_collisions() {
    let mut schema = Schema::default();
    field(&mut schema, &["title"], LogicalType::String, false, false);
    assert!(
        schema
            .register_relation(relation(
                &["title"],
                "entries",
                RelationCardinality::One,
                "bad"
            ))
            .unwrap_err()
            .contains("ID field")
    );
    let root = fixture("invalid");
    assert!(
        Repository::open_with_options(
            &root,
            definitions("missing"),
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            }
        )
        .err()
        .unwrap()
        .contains("target collection is unknown")
    );
    let mut defs = definitions("entries");
    defs[2]
        .schema
        .relations
        .get_mut(&vec!["items".into(), "entry".into()])
        .unwrap()
        .reverse_alias = Some("mentions".into());
    assert!(
        Repository::open_with_options(
            &root,
            defs,
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            }
        )
        .err()
        .unwrap()
        .contains("duplicate reverse relation alias")
    );
    std::fs::write(root.join("entries/duplicate.json"), r#"{"id":"A"}"#).unwrap();
    assert!(
        Repository::open_with_options(
            &root,
            definitions("entries"),
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            }
        )
        .err()
        .unwrap()
        .contains("duplicate record ID")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn jsonl_siblings_and_shared_target_file_keep_relation_ownership() {
    let root = std::env::temp_dir().join(format!("objekte-relation-jsonl-{}", std::process::id()));
    for collection in ["entries", "people", "mentions"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
    }
    std::fs::write(
        root.join("entries/records.jsonl"),
        "{\"id\":\"A\"}\n{\"id\":\"B\"}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("mentions/records.jsonl"),
        "{\"id\":\"m1\",\"entry\":\"A\"}\n{\"broken\":\n{\"id\":\"m2\",\"entry\":\"B\"}\n",
    )
    .unwrap();
    let mut defs = definitions("entries");
    defs[0].format = SourceFormat::JsonLines;
    defs[2].format = SourceFormat::JsonLines;
    let repo = Repository::open_with_options(
        &root,
        defs,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(repo.build_parses(), 4);
    assert_eq!(repo.build_failed_parses(), 1);
    let entries = &repo.collections()[0];
    let mentions = &repo.collections()[2];
    assert_eq!(
        entries.generation.records[0].file,
        entries.generation.records[1].file
    );
    assert_eq!(mentions.generation.records.len(), 3);
    assert!(
        mentions
            .generation
            .records
            .iter()
            .all(|record| record.file == mentions.generation.records[0].file)
    );
    let relation = repo.relation_index("mentions", &["entry".into()]).unwrap();
    for (source_id, target_id) in [("m1", "A"), ("m2", "B")] {
        let source = mentions.id_lookup[source_id];
        assert_eq!(
            relation.edges(source).unwrap()[0].target,
            RelationTarget::Resolved(entries.id_lookup[target_id])
        );
        assert_eq!(
            repo.relation_reverse_sources(relation, target_id)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [source.0]
        );
    }
    let fallback = mentions
        .generation
        .records
        .iter()
        .find(|record| record.class == RecordClass::RawFallback)
        .unwrap()
        .slot;
    assert!(relation.unavailable().contains(fallback.0));
    assert!(relation.edges(fallback).unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reference_to_fallback_target_resolves_but_target_value_is_unavailable() {
    let root = std::env::temp_dir().join(format!(
        "objekte-relation-fallback-target-{}",
        std::process::id()
    ));
    for collection in ["entries", "people", "mentions"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
    }
    std::fs::write(root.join("entries/broken.json"), "{broken").unwrap();
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        ..Default::default()
    };
    let initial = Repository::open_with_options(&root, definitions("entries"), options).unwrap();
    let target_id = initial.collections()[0].generation.records[0].id.0.clone();
    assert!(target_id.starts_with("~fallback:"));
    std::fs::write(
        root.join("mentions/link.json"),
        serde_json::json!({"id":"link","entry":target_id}).to_string(),
    )
    .unwrap();
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    for restored in [false, true] {
        let repo = Repository::open_with_options(&root, definitions("entries"), options).unwrap();
        assert_eq!(repo.startup().restored, restored);
        let source = &repo.collections()[2];
        let target = &repo.collections()[0];
        let relation = repo.relation_index("mentions", &["entry".into()]).unwrap();
        let source_slot = source.id_lookup["link"];
        let target_slot = target.id_lookup[target_id.as_str()];
        assert_eq!(
            relation.edges(source_slot).unwrap()[0].target,
            RelationTarget::Resolved(target_slot)
        );
        assert_eq!(
            target.generation.records[target_slot.0 as usize].class,
            RecordClass::RawFallback
        );
        assert!(!target.logical_records.contains(target_slot.0));
        assert_eq!(relation.unresolved_count(), 0);
        assert!(relation.unresolved_references().next().is_none());
        assert_eq!(
            repo.relation_reverse_sources(relation, &target_id)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [source_slot.0]
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
