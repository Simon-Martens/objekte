use super::*;
use redb::ReadableDatabase;
use crate::bigram::BigramIndex;
use crate::workload::BigramAlphabet;
use crate::build::{FilePreparationBinding, SegmentAdmission, SourceFileInput, prepare_file_build};
use crate::index_store_keys as keys;
use crate::inference::{InferenceConfig, InferenceMode, InferencePolicy};
use crate::inventory::{ActiveRecords, SourceRevision};
use crate::model::{CollectionId, FieldPath, FileId, LogicalType, RecordSlot};
use crate::reference::FieldDefinition;
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::CollectionDefinition;
use crate::source::{SourceFormat, SourceRegistry};

fn append_fixture_shard(tag: &str, file_id: u32, record_count: usize) -> IndexedCollectionSegment {
    append_fixture_shard_records(tag, file_id, &(0..record_count).collect::<Vec<_>>())
}

fn append_fixture_shard_records(
    tag: &str,
    file_id: u32,
    numbers: &[usize],
) -> IndexedCollectionSegment {
    let root = std::env::temp_dir().join(format!(
        "objekte-append-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut definition = CollectionDefinition::folder("data", ".", SourceFormat::JsonLines);
    for (path, kind, multi) in [
        ("name", LogicalType::String, false),
        ("n", LogicalType::Number, false),
        (
            "tags",
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
        ),
        ("ref", LogicalType::Id, false),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: false,
                nullable: false,
                indexed: true,
                multi,
                element_nullable: vec![],
            })
            .unwrap();
    }
    definition
        .schema
        .register_relation(RelationDefinition {
            path: vec!["ref".into()],
            target_collection: "target".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    definition
        .schema
        .register_custom(
            vec!["name".into()],
            crate::reference::CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: crate::reference::Transform::CaseFold,
            },
        )
        .unwrap();
    definition.sort_fields.push(FieldPath::parse("name").unwrap());
    definition.sort_fields.push(FieldPath::parse("n").unwrap());
    definition
        .schema
        .register_composite_sort(vec![
            crate::workload::SortField {
                field: "n".into(),
                descending: false,
                expression: None,
            },
            crate::workload::SortField {
                field: "name".into(),
                descending: true,
                expression: None,
            },
        ])
        .unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::JsonLines)
        .unwrap();
    let name = format!("{tag}.jsonl");
    let body = numbers
        .iter()
        .map(|number| {
            format!(
                "{{\"name\":\"name-{number}\",\"n\":{number},\"tags\":[\"tag-a\",\"tag-b\"],\"ref\":\"target-{}\",\"extra\":{number}}}\n",
                number % 5
            )
        })
        .collect::<String>();
    std::fs::write(root.join(&name), body).unwrap();
    let input = SourceFileInput {
        id: FileId(file_id),
        relative_path: name.clone().into(),
        revision: SourceRevision::from_metadata(&std::fs::metadata(root.join(&name)).unwrap())
            .unwrap(),
        created_ns: None,
    };
    let binding = FilePreparationBinding {
        collection: CollectionId(0),
        definition: &definition,
        source: &source,
    };
    let inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        policy: InferencePolicy::default(),
    };
    let prepared = prepare_file_build(
        &root,
        input.clone(),
        &[binding],
        BigramAlphabet::Ascii,
        inference,
        1,
    )
    .unwrap();
    let mut admission = SegmentAdmission::new(CollectionId(0), &source);
    admission
        .admit(&input, prepared.collections.into_iter().next().unwrap())
        .unwrap();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let indexed = admission
        .finish()
        .into_indexed(&definition, BigramAlphabet::Ascii, inference, 1, &pool)
        .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    indexed
}

fn assert_shards_equal(actual: &IndexedCollectionSegment, expected: &IndexedCollectionSegment) {
    assert_eq!(actual.record_count(), expected.record_count());
    for slot in 0..actual.record_count() {
        assert_eq!(
            format!("{:?}", actual.record(RecordSlot(slot as u32))),
            format!("{:?}", expected.record(RecordSlot(slot as u32)))
        );
    }
    assert_eq!(actual.file_count(), expected.file_count());
    for position in 0..actual.file_count() {
        let left = actual.file_at(position).unwrap();
        let right = expected.file_at(position).unwrap();
        assert_eq!(format!("{:?}", left.entry), format!("{:?}", right.entry));
        assert_eq!(left.records, right.records);
    }
    let ids = |shard: &IndexedCollectionSegment| {
        shard
            .ids_iter()
            .map(|(id, slot)| (id.clone(), slot))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    assert_eq!(ids(actual), ids(expected));
    assert_eq!(actual.identity_order().to_vec(), expected.identity_order().to_vec());
    assert_eq!(actual.logical_records(), expected.logical_records());
    assert_eq!(actual.empty_array_records(), expected.empty_array_records());
    for (path, field) in &expected.postings.fields {
        let other = &actual.postings.fields[path];
        assert_eq!(other.value_entries(), field.value_entries());
        assert_eq!(other.length_entries(), field.length_entries());
        assert_eq!(
            other.structural.as_ref().unwrap().entries(),
            field.structural.as_ref().unwrap().entries()
        );
        assert_eq!(other.presence, field.presence);
        assert_eq!(other.nulls, field.nulls);
        assert_eq!(other.empty_selections, field.empty_selections);
        assert_eq!(other.covered_records, field.covered_records);
        assert_eq!(other.stats.present_count, field.stats.present_count);
        assert_eq!(other.stats.null_count, field.stats.null_count);
        assert_eq!(other.stats.distinct_count, field.stats.distinct_count);
        assert_eq!(other.stats.min_key, field.stats.min_key);
        assert_eq!(other.stats.max_key, field.stats.max_key);
    }
    assert_eq!(actual.custom_postings.len(), expected.custom_postings.len());
    for (key, custom) in &expected.custom_postings {
        let other = &actual.custom_postings[key];
        assert_eq!(other.index.value_entries(), custom.index.value_entries());
        assert_eq!(other.complete, custom.complete);
    }
    match (&actual.inference, &expected.inference) {
        (None, None) => {}
        (Some(left), Some(right)) => {
            assert_eq!(format!("{:?}", left.report), format!("{:?}", right.report));
            let paths = left
                .indexes
                .keys()
                .chain(right.indexes.keys())
                .collect::<std::collections::BTreeSet<_>>();
            for path in paths {
                assert_eq!(left.indexes.get(path).map(|i| i.value_entries()), right.indexes.get(path).map(|i| i.value_entries()));
                assert_eq!(left.empty_selections.get(path), right.empty_selections.get(path));
                assert_eq!(left.short_string_coverage.get(path), right.short_string_coverage.get(path));
            }
        }
        _ => panic!("inference presence differs"),
    }
    let grams = |index: &BigramIndex| {
        (
            index.normal_entries(),
            index.skip_entries(),
            index.record_count,
        )
    };
    assert_eq!(grams(&actual.raw), grams(&expected.raw));
    assert_eq!(grams(&actual.fallback_text), grams(&expected.fallback_text));
    assert_eq!(actual.custom_text.len(), expected.custom_text.len());
    for (key, index) in &expected.custom_text {
        assert_eq!(grams(&actual.custom_text[key]), grams(index));
    }
    assert_eq!(
        actual.sorting.columns.len(),
        expected.sorting.columns.len()
    );
    for (path, column) in &expected.sorting.columns {
        let other = &actual.sorting.columns[path];
        assert_eq!(
            other.keys_iter().cloned().collect::<Vec<_>>(),
            column.keys_iter().cloned().collect::<Vec<_>>()
        );
        assert_eq!(other.ascending().to_vec(), column.ascending().to_vec());
        assert_eq!(other.descending().to_vec(), column.descending().to_vec());
    }
    assert_eq!(
        actual.sorting.composites.len(),
        expected.sorting.composites.len()
    );
    for (left, right) in actual.sorting.composites.iter().zip(&expected.sorting.composites) {
        assert_eq!(left.order().to_vec(), right.order().to_vec());
    }
    assert_eq!(actual.relations().len(), expected.relations().len());
    for (left, right) in actual.relations().iter().zip(expected.relations()) {
        assert_eq!(left.present(), right.present());
        assert_eq!(left.nulls(), right.nulls());
        assert_eq!(left.empty_arrays(), right.empty_arrays());
        assert_eq!(left.unavailable(), right.unavailable());
        assert_eq!(
            left.references().collect::<Vec<_>>(),
            right.references().collect::<Vec<_>>()
        );
    }
    assert_eq!(actual.record_parses, expected.record_parses);
    assert_eq!(actual.failed_parses, expected.failed_parses);
}


fn in_memory_store() -> redb::Database {
    redb::Database::builder()
        .create_with_backend(redb::backends::InMemoryBackend::new())
        .unwrap()
}

#[test]
fn shard_round_trip_native() {
    let shard = append_fixture_shard("round-native", 1, 40);
    let mut expected = shard.clone();
    expected.recompute_accounting();
    let active = ActiveRecords::all(shard.record_count()).unwrap();
    let db = in_memory_store();
    {
        let write = db.begin_write().unwrap();
        write_shard(&write, 0, 1, 7, &shard, &active).unwrap();
        write.commit().unwrap();
    }
    let read = db.begin_read().unwrap();
    let (actual, actual_active, _) = read_shard(&read, 0, 1).unwrap();
    assert_shards_equal(&actual, &expected);
    assert_eq!(actual_active.bitmap(), active.bitmap());
    if !cfg!(feature = "force-shared-backing") {
        assert_eq!(
            actual.accounting.retained_bytes_estimated,
            expected.accounting.retained_bytes_estimated
        );
    }
}

#[test]
fn shard_round_trip_shared() {
    let base = append_fixture_shard("round-shared", 1, 30);
    let d1 = append_fixture_shard("round-shared-1", 2, 5);
    let d2 = append_fixture_shard("round-shared-2", 3, 4);
    let d3 = append_fixture_shard("round-shared-3", 4, 6);
    let mut main = base.to_shared();
    let mut active = ActiveRecords::all(base.record_count()).unwrap();
    for delta in [&d1, &d2, &d3] {
        let appended = main.append(&active, delta).unwrap();
        active = appended
            .index
            .accounting
            .retained_bytes_estimated
            .checked_sub(0)
            .map(|_| active.concat(&ActiveRecords::all(delta.record_count()).unwrap()).unwrap())
            .unwrap();
        main = appended.index;
    }
    let mut expected = main.clone();
    expected.recompute_accounting();
    let db = in_memory_store();
    {
        let write = db.begin_write().unwrap();
        write_shard(&write, 0, 9, 3, &main, &active).unwrap();
        write.commit().unwrap();
    }
    let read = db.begin_read().unwrap();
    let (actual, actual_active, _) = read_shard(&read, 0, 9).unwrap();
    assert_shards_equal(&actual, &expected);
    assert_eq!(actual_active.bitmap(), active.bitmap());
}

#[test]
fn shard_round_trip_keeps_the_active_mask() {
    let shard = append_fixture_shard("round-active", 1, 10);
    let active = ActiveRecords::all(shard.record_count())
        .unwrap()
        .retired(&[RecordSlot(1), RecordSlot(4), RecordSlot(9)])
        .unwrap();
    let db = in_memory_store();
    {
        let write = db.begin_write().unwrap();
        write_shard(&write, 0, 2, 1, &shard, &active).unwrap();
        write.commit().unwrap();
    }
    let read = db.begin_read().unwrap();
    let (_, actual, _) = read_shard(&read, 0, 2).unwrap();
    assert_eq!(actual.bitmap(), active.bitmap());
    assert_eq!(actual.len(), active.len());
}

#[test]
fn deleting_a_shard_prefix_removes_every_row() {
    let first = append_fixture_shard("delete-first", 1, 8);
    let second = append_fixture_shard("delete-second", 2, 8);
    let active = ActiveRecords::all(8).unwrap();
    let db = in_memory_store();
    {
        let write = db.begin_write().unwrap();
        write_shard(&write, 0, 1, 1, &first, &active).unwrap();
        write_shard(&write, 0, 2, 2, &second, &active).unwrap();
        write.commit().unwrap();
    }
    {
        let write = db.begin_write().unwrap();
        delete_shard_prefix(&write, 0, 1).unwrap();
        write.commit().unwrap();
    }
    let read = db.begin_read().unwrap();
    let first_prefix = keys::prefix(0, 1);
    let second_prefix = keys::prefix(0, 2);
    let first_end = keys::prefix_end(&first_prefix).unwrap();
    let second_end = keys::prefix_end(&second_prefix).unwrap();
    for definition in [
        keys::SHARDS,
        keys::ACTIVE,
        keys::FILES,
        keys::RECORDS,
        keys::POSTING_FIELDS,
        keys::POSTINGS,
        keys::BITMAPS,
        keys::SORT_COLUMNS,
        keys::SORT_KEYS,
        keys::COMPOSITES,
        keys::RELATIONS,
        keys::REFERENCES,
        keys::INFERENCE,
    ] {
        let table = read.open_table(definition).unwrap();
        assert_eq!(
            table.range::<&[u8]>(first_prefix.as_slice()..first_end.as_slice()).unwrap().count(),
            0
        );
        assert!(
            table
                .range::<&[u8]>(second_prefix.as_slice()..second_end.as_slice())
                .unwrap()
                .count()
                > 0
        );
    }
}

#[test]
fn diff_then_read_equals_full_write() {
    let base = append_fixture_shard("diff-base", 1, 40);
    let delta = append_fixture_shard("diff-delta", 2, 3);
    let m1 = base.to_shared();
    let active = ActiveRecords::all(base.record_count()).unwrap();
    let m2 = m1.append(&active, &delta).unwrap().index;
    let active2 = ActiveRecords::all(m2.record_count()).unwrap();
    let db = in_memory_store();
    {
        let write = db.begin_write().unwrap();
        write_shard(&write, 0, 1, 1, &m1, &active).unwrap();
        write.commit().unwrap();
    }
    {
        let write = db.begin_write().unwrap();
        diff_main(&write, 0, 1, &m1, &m2, &active, &active2).unwrap();
        write.commit().unwrap();
    }
    let read = db.begin_read().unwrap();
    let (actual, actual_active, _) = read_shard(&read, 0, 1).unwrap();
    assert_shards_equal(&actual, &m2);
    assert_eq!(actual_active.bitmap(), active2.bitmap());
}

#[test]
fn diff_writes_are_proportional_to_the_delta() {
    let base = append_fixture_shard("diff-large", 1, 5_000);
    let delta = append_fixture_shard("diff-small", 2, 1);
    let m1 = base.to_shared();
    let active = ActiveRecords::all(base.record_count()).unwrap();
    let m2 = m1.append(&active, &delta).unwrap().index;
    let active2 = ActiveRecords::all(m2.record_count()).unwrap();
    let mut distinct = std::collections::BTreeSet::new();
    for (path, index) in &delta.postings.fields {
        for (key, _) in index.value_entries() {
            distinct.insert(format!("v:{path:?}:{key:?}"));
        }
        for (key, _) in index.length_entries() {
            distinct.insert(format!("l:{path:?}:{key:?}"));
        }
    }
    for (key, _) in delta.raw.normal_entries() {
        distinct.insert(format!("gn:{key}"));
    }
    for (key, _) in delta.raw.skip_entries() {
        distinct.insert(format!("gs:{key}"));
    }
    let db = in_memory_store();
    {
        let write = db.begin_write().unwrap();
        write_shard(&write, 0, 1, 1, &m1, &active).unwrap();
        write.commit().unwrap();
    }
    let report = {
        let write = db.begin_write().unwrap();
        let report = diff_main(&write, 0, 1, &m1, &m2, &active, &active2).unwrap();
        write.commit().unwrap();
        report
    };
    assert!(
        report.chunks_written <= 4 * distinct.len() as u64,
        "chunks {} distinct {}",
        report.chunks_written,
        distinct.len()
    );
    assert!(
        report.rows_written <= 10 + m2.file_count() as u64,
        "rows {} files {}",
        report.rows_written,
        m2.file_count()
    );
    let read = db.begin_read().unwrap();
    let (actual, _, _) = read_shard(&read, 0, 1).unwrap();
    assert_shards_equal(&actual, &m2);
}
