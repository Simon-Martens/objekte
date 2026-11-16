//! Focused unit tests for the independent stable-index prototype.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::inventory::RecordEntry;
use crate::model::{CollectionId, FileId, LogicalType, Number, RecordId, RecordSlot};
use crate::posting::ScalarKey;
use crate::reference::FieldDefinition;
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
use crate::source::SourceFormat;

use super::codec::{ComponentKey, PathKey, SortEntryKey};
use super::memory;
use super::*;

fn temp_root(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "objekte-icss-{label}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("create fixture root");
    root
}

fn field(path: &[&str], kind: LogicalType, multi: bool, indexed: bool) -> FieldDefinition {
    FieldDefinition {
        path: path.iter().map(|part| part.to_string()).collect(),
        kind,
        required: false,
        nullable: true,
        indexed,
        multi,
        element_nullable: Vec::new(),
    }
}

struct Fixture {
    root: PathBuf,
    repository: Repository,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fixture(label: &str) -> Fixture {
    let root = temp_root(label);
    std::fs::create_dir_all(root.join("data/nested")).expect("data directory");
    std::fs::write(
        root.join("data/a.jsonl"),
        b"{\"name\":\"alpha\",\"age\":1,\"tags\":[\"x\",\"y\"]}\n{\"name\":\"beta\",\"age\":2,\"tags\":[\"x\"]}\n",
    )
    .expect("write a");
    std::fs::write(
        root.join("data/nested/c.jsonl"),
        b"{\"name\":\"alpha\",\"age\":1,\"tags\":[]}\n{\"name\":\"gamma\",\"age\":10,\"tags\":[\"z\"]}\n",
    )
    .expect("write c");
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("name field");
    definition
        .schema
        .register_field(field(&["age"], LogicalType::Number, false, true))
        .expect("age field");
    definition
        .schema
        .register_field(field(
            &["tags"],
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
            true,
        ))
        .expect("tags field");
    definition.sort_fields = vec![crate::model::FieldPath::parse("age").expect("sort path")];
    let repository =
        Repository::open_with_options(&root, vec![definition], RepositoryOptions::default())
            .expect("repository");
    Fixture { root, repository }
}

fn age_component(prototype: &StablePrototype) -> super::ComponentId {
    let key = super::FieldComponentKey {
        path: vec!["age".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    prototype
        .field_components(CollectionId(0), &key)
        .and_then(|entry| entry.values)
        .expect("age value component")
}

#[test]
fn native_repository_builds_independent_components_and_validates_exactly() {
    let fixture = fixture("validate");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    assert!(
        prototype.component_descriptors().count() >= 10,
        "expected independent components for catalogs, fields, text and sorts"
    );
    let report = prototype
        .validate_against_repository(&fixture.repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);
    assert_eq!(report.files, 2);
    assert_eq!(report.records, 4);
    assert!(report.posting_keys > 0);
    assert!(report.gram_keys > 0);
    assert!(report.sort_entries > 0);

    let age = super::FieldComponentKey {
        path: vec!["age".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    let one = prototype
        .typed_posting(
            CollectionId(0),
            &age,
            &ScalarKey::Number(Number::integer(1)),
        )
        .expect("one")
        .complete()
        .expect("complete one");
    assert_eq!(one.len(), 2);
    let range = prototype
        .typed_range_slots(
            CollectionId(0),
            &age,
            std::ops::Bound::Included(ScalarKey::Number(Number::integer(1))),
            std::ops::Bound::Excluded(ScalarKey::Number(Number::integer(10))),
            false,
        )
        .expect("range")
        .complete()
        .expect("complete range");
    assert_eq!(range.len(), 3);

    let path = fixture.repository.files().files[0].relative_path.clone();
    let refs = prototype.records_for_path(&path).expect("path refs");
    assert_eq!(refs.len(), 2);
    let identities = prototype.identities_for_path(&path).expect("identities");
    assert_eq!(identities.len(), 2);
    let first_id = fixture.repository.collections()[0].generation.records[0]
        .id
        .clone();
    assert_eq!(
        prototype
            .records_for_identity(CollectionId(0), &first_id)
            .expect("identity")
            .len(),
        1
    );
}

#[test]
fn standalone_round_trip_without_manifest_supports_mutation() {
    let fixture = fixture("standalone");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let age = age_component(&prototype);
    let root = temp_root("standalone-files");
    let first = root.join("age-values.icss");
    let info = prototype.save_index(age, &first).expect("save index");
    assert_eq!(info.entry_count, 3);

    let moved_dir = temp_root("standalone-moved");
    let moved = moved_dir.join("renamed.bin");
    std::fs::copy(&first, &moved).expect("copy standalone");
    std::fs::remove_file(&first).expect("remove original");
    let mut standalone = StablePrototype::open_index(&moved).expect("open standalone");
    assert_eq!(standalone.entry_count(), 3);
    let one = ComponentKey::Scalar(ScalarKey::Number(Number::integer(1)));
    let posting = standalone.raw_posting(&one).expect("posting").expect("key");
    assert_eq!(posting.len(), 2);

    let absent = ComponentKey::Scalar(ScalarKey::String("absent".into()));
    assert!(standalone.raw_get(&absent).expect("get").is_none());
    standalone
        .raw_update_posting(
            &absent,
            &RoaringBitmap::from_iter([99]),
            &RoaringBitmap::new(),
        )
        .expect("insert posting");
    assert!(standalone.is_dirty());
    assert!(standalone.descriptor().detached);

    let large = RoaringBitmap::from_iter(0..4096);
    standalone
        .raw_update_posting(&one, &large, &RoaringBitmap::new())
        .expect("replace larger");
    standalone
        .raw_update_posting(&one, &RoaringBitmap::new(), &large)
        .expect("replace smaller");
    standalone
        .raw_update_posting(&one, &RoaringBitmap::from_iter([1]), &RoaringBitmap::new())
        .expect("remove missing unchanged");
    standalone
        .raw_remove(&absent)
        .expect("remove missing")
        .expect("absent was present");
    standalone
        .raw_remove(&absent)
        .expect("remove missing again");
    for index in 0..1_000u64 {
        standalone
            .raw_insert(ComponentKey::Gram(index), StoredBytes::empty())
            .expect_err("gram keys are not valid for a value component");
    }

    let second = moved_dir.join("mutated.icss");
    standalone.save_as(&second).expect("save as");
    assert!(!standalone.is_dirty());
    drop(standalone);
    let reopened = StablePrototype::open_index(&second).expect("reopen standalone");
    assert_eq!(
        reopened
            .raw_posting(&one)
            .expect("reopened posting")
            .expect("key")
            .len(),
        1
    );
    assert!(reopened.descriptor().detached);
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&moved_dir);
}

#[test]
fn standalone_variable_values_split_merge_and_survive_reopen() {
    let fixture = fixture("standalone-split");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let age = age_component(&prototype);
    let root = temp_root("split-files");
    let first = root.join("values.icss");
    prototype.save_index(age, &first).expect("save");
    let mut standalone = StablePrototype::open_index(&first).expect("open");
    let key = |index: u64| ComponentKey::Scalar(ScalarKey::String(format!("key-{index:06}")));
    for index in 0..1_500u64 {
        standalone
            .raw_insert(
                key(index),
                memory::bitmap_value(&RoaringBitmap::from_iter([index as u32])).expect("bitmap"),
            )
            .expect("insert");
    }
    for index in 0..1_400u64 {
        standalone.raw_remove(&key(index)).expect("remove");
    }
    let saved = root.join("split.icss");
    standalone.save_as(&saved).expect("save");
    drop(standalone);
    let reopened = StablePrototype::open_index(&saved).expect("reopen");
    for index in 0..1_400u64 {
        assert!(reopened.raw_get(&key(index)).expect("get").is_none());
    }
    for index in 1_400..1_500u64 {
        assert!(reopened.raw_get(&key(index)).expect("get").is_some());
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn generation_is_lazy_and_old_generations_survive_new_saves() {
    let fixture = fixture("generation");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("generations");
    let first = root.join("generation-1");
    prototype.save_snapshot(&first).expect("save first");
    assert!(first.join("manifest.json").is_file());
    let descriptor_count = prototype.component_descriptors().count();
    let first_manifest = super::read_manifest(&first).expect("manifest");
    assert_eq!(first_manifest.components.len(), descriptor_count);

    let mut opened = StablePrototype::open_snapshot(&first).expect("open generation");
    assert_eq!(opened.counters().manifest_opens, 1);
    assert_eq!(opened.counters().component_loads, 0);
    let file_id = FileId(0);
    let file = opened.file_by_id(file_id).expect("file").expect("present");
    assert_eq!(opened.counters().component_loads, 1);
    assert!(file.relative_path.as_path().starts_with("data"));
    let first_id = fixture.repository.collections()[0].generation.records[0]
        .id
        .clone();
    assert_eq!(
        opened
            .records_for_identity(CollectionId(0), &first_id)
            .expect("identity lookup")
            .len(),
        1
    );
    assert!(opened.counters().component_loads >= 3);

    let one_manifest = first.join("manifest.json");
    let component_path = first.join("c000000/records.icss");
    assert!(component_path.is_file());
    let mut copied = StablePrototype::open_index(&component_path).expect("open member");
    assert_eq!(copied.entry_count(), 4);
    let save_copy = root.join("copied-records.icss");
    copied.save_as(&save_copy).expect("save copy");
    std::fs::remove_dir_all(&first).expect("remove generation");
    assert!(!one_manifest.exists());
    let reopened = StablePrototype::open_index(&save_copy).expect("open copied member");
    assert_eq!(reopened.entry_count(), 4);

    // Save a second generation after an in-memory mutation.
    let second = root.join("generation-2");
    let first_records = fixture.repository.collections()[0]
        .generation
        .records
        .clone();
    let old_file = fixture.repository.files().files[0].clone();
    let old_path = old_file.relative_path.to_path_buf();
    let mut mutated = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let replacement = mutated
        .replace_file(
            &old_path,
            old_file,
            CollectionId(0),
            first_records
                .into_iter()
                .filter(|record| record.file == FileId(0))
                .collect(),
        )
        .expect("replace file");
    assert!(!replacement.is_empty());
    mutated.save_snapshot(&second).expect("save second");

    let mut second_opened = StablePrototype::open_snapshot(&second).expect("open second");
    let refs = second_opened.records_for_path(&old_path).expect("path");
    assert_eq!(refs.len(), 2);
    assert_eq!(
        second_opened
            .live_slots(CollectionId(0))
            .expect("live")
            .len(),
        4
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn corruption_and_identity_mismatches_are_rejected() {
    let fixture = fixture("corruption");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let age = age_component(&prototype);
    let root = temp_root("corruption-files");
    let good = root.join("good.icss");
    prototype.save_index(age, &good).expect("save");
    let bytes = std::fs::read(&good).expect("read");

    let truncated = root.join("truncated.icss");
    std::fs::write(&truncated, &bytes[..bytes.len() - 17]).expect("write truncated");
    assert!(StablePrototype::open_index(&truncated).is_err());

    let flipped = root.join("flipped.icss");
    let mut changed = bytes.clone();
    *changed.last_mut().expect("payload") ^= 1;
    std::fs::write(&flipped, &changed).expect("write flipped");
    assert!(StablePrototype::open_index(&flipped).is_err());

    let bad_magic = root.join("bad-magic.icss");
    let mut changed = bytes.clone();
    changed[0] ^= 1;
    std::fs::write(&bad_magic, &changed).expect("write bad magic");
    assert!(StablePrototype::open_index(&bad_magic).is_err());

    let oversized_descriptor = root.join("oversized-descriptor.icss");
    let mut changed = bytes.clone();
    changed[120..124].copy_from_slice(&(codec::MAX_DESCRIPTOR_BYTES as u32 + 1).to_le_bytes());
    std::fs::write(&oversized_descriptor, &changed).expect("write oversized");
    assert!(StablePrototype::open_index(&oversized_descriptor).is_err());

    let descriptor_flip = root.join("descriptor-flip.icss");
    let mut changed = bytes.clone();
    changed[128] ^= 1;
    std::fs::write(&descriptor_flip, &changed).expect("write descriptor flip");
    assert!(StablePrototype::open_index(&descriptor_flip).is_err());

    let wrong_id = root.join("wrong-id.icss");
    let mut changed = bytes.clone();
    let id = u64::from_le_bytes(changed[16..24].try_into().expect("id"));
    changed[16..24].copy_from_slice(&(id + 1).to_le_bytes());
    std::fs::write(&wrong_id, &changed).expect("write wrong id");
    assert!(StablePrototype::open_index(&wrong_id).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn sort_order_and_path_identity_use_native_semantics() {
    let fixture = fixture("sort");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let sort_descriptor = prototype
        .component_descriptors()
        .find(|descriptor| descriptor.kind == ComponentKind::SortAscending)
        .expect("sort descriptor")
        .clone();
    let key = super::sort_component_key(&sort_descriptor);
    let ascending = prototype
        .sort_slots(CollectionId(0), &key, false)
        .expect("ascending");
    let descending = prototype
        .sort_slots(CollectionId(0), &key, true)
        .expect("descending");
    assert_eq!(ascending.len(), 4);
    assert_eq!(descending.len(), 4);
    let native = fixture.repository.collections()[0]
        .sorting
        .columns
        .values()
        .next()
        .expect("native sort");
    assert_eq!(ascending, native.ascending);
    assert_eq!(descending, native.descending);
    let page = prototype
        .sort_page(CollectionId(0), &key, false, 1, 2)
        .expect("page");
    assert_eq!(page, native.ascending[1..3].to_vec());

    let sort_key = |slot: u32| {
        ComponentKey::Sort(SortEntryKey {
            options: sort_descriptor.ordering.as_ref().expect("ordering").options,
            descending: false,
            key: crate::sorting::SortKey::Missing,
            tie_break_id: RecordId(format!("{slot}").into()),
            slot: RecordSlot(slot),
        })
    };
    assert_eq!(
        sort_key(1).tag(),
        ComponentKey::Sort(SortEntryKey {
            options: sort_descriptor.ordering.as_ref().unwrap().options,
            descending: false,
            key: crate::sorting::SortKey::Unavailable,
            tie_break_id: RecordId("2".into()),
            slot: RecordSlot(2),
        })
        .tag()
    );
}

#[test]
fn file_lifecycle_updates_catalogs_and_live_domains() {
    let fixture = fixture("lifecycle");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let file = fixture.repository.files().files[0].clone();
    let path = file.relative_path.to_path_buf();
    let records = fixture.repository.collections()[0]
        .generation
        .records
        .clone();
    let file_records = records
        .iter()
        .filter(|record| record.file == file.id)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(prototype.records_for_path(&path).expect("refs").len(), 2);
    let deleted = prototype.delete_file_by_path(&path).expect("delete");
    assert_eq!(deleted.len(), 2);
    assert!(prototype.records_for_path(&path).expect("refs").is_empty());
    assert_eq!(
        prototype.live_slots(CollectionId(0)).expect("live").len(),
        2
    );
    for record in &file_records {
        assert!(
            prototype
                .record_by_slot(CollectionId(0), record.slot)
                .expect("record")
                .is_none()
        );
    }

    let replaced = prototype
        .replace_file(&path, file, CollectionId(0), file_records)
        .expect("replace");
    assert_eq!(replaced.len(), 2);
    assert_eq!(prototype.records_for_path(&path).expect("refs").len(), 2);
    assert_eq!(
        prototype.live_slots(CollectionId(0)).expect("live").len(),
        4
    );

    let root = temp_root("lifecycle-generation");
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    drop(prototype);
    let mut reopened = StablePrototype::open_snapshot(&generation).expect("open");
    assert_eq!(reopened.records_for_path(&path).expect("refs").len(), 2);
    assert_eq!(reopened.live_slots(CollectionId(0)).expect("live").len(), 4);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn open_index_rejects_keys_from_another_component_kind() {
    let fixture = fixture("key-kind");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let age = age_component(&prototype);
    let root = temp_root("key-kind-files");
    let path = root.join("values.icss");
    prototype.save_index(age, &path).expect("save");
    let mut standalone = StablePrototype::open_index(&path).expect("open");
    assert!(
        standalone
            .raw_insert(ComponentKey::Gram(1), StoredBytes::empty())
            .is_err()
    );
    assert!(standalone.raw_get(&ComponentKey::RecordSlot(1)).is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn path_keys_compare_losslessly_without_normalization() {
    let a = PathKey::from_path(Path::new("Dir/Name.json")).expect("a");
    let b = PathKey::from_path(Path::new("dir/name.json")).expect("b");
    assert_ne!(a, b);
    let set = BTreeSet::from([a.clone(), b.clone()]);
    assert_eq!(set.len(), 2);
    assert_eq!(a.to_path().expect("path"), Path::new("Dir/Name.json"));
}

#[test]
fn standalone_repository_semantics_require_dependencies() {
    let fixture = fixture("standalone-dependency");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let age = age_component(&prototype);
    let root = temp_root("standalone-dependency-files");
    let path = root.join("values.icss");
    prototype.save_index(age, &path).expect("save");
    let standalone = StablePrototype::open_index(&path).expect("open");
    let key = ComponentKey::Scalar(ScalarKey::Number(Number::integer(1)));
    assert!(standalone.raw_posting(&key).expect("raw").is_some());
    assert!(matches!(
        standalone.live_posting(&key),
        Err(StableIndexError::MissingDependency(_))
    ));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn first_field_query_does_not_load_unrelated_field_payloads() {
    let fixture = fixture("lazy-query");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("lazy-query-generation");
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    let mut opened = StablePrototype::open_snapshot(&generation).expect("open");
    let age_key = super::FieldComponentKey {
        path: vec!["age".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    let name_key = super::FieldComponentKey {
        path: vec!["name".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    let age_component = opened
        .field_components(CollectionId(0), &age_key)
        .and_then(|entry| entry.values)
        .expect("age values");
    let name_component = opened
        .field_components(CollectionId(0), &name_key)
        .and_then(|entry| entry.values)
        .expect("name values");
    opened
        .typed_posting(
            CollectionId(0),
            &age_key,
            &ScalarKey::Number(Number::integer(1)),
        )
        .expect("query age");
    assert_eq!(
        opened
            .component_statistics(age_component)
            .expect("age statistics")
            .state,
        "loaded"
    );
    assert_eq!(
        opened
            .component_statistics(name_component)
            .expect("name statistics")
            .state,
        "unloaded"
    );
    // Querying a value loads its own dictionary and the small live domain,
    // never an unrelated field or the full source record table.
    let loaded = opened
        .component_descriptors()
        .map(|descriptor| descriptor.id)
        .filter(|id| {
            opened
                .component_statistics(*id)
                .map(|statistics| statistics.state != "unloaded")
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    assert_eq!(loaded.len(), 2, "unexpected loaded components: {loaded:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn snapshot_copy_of_unloaded_components_does_not_load_them() {
    let fixture = fixture("unloaded-copy");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("unloaded-copy-generations");
    let first = root.join("first");
    prototype.save_snapshot(&first).expect("first");
    let mut opened = StablePrototype::open_snapshot(&first).expect("open");
    let file_id = FileId(0);
    opened.file_by_id(file_id).expect("file").expect("present");
    assert_eq!(opened.counters().component_loads, 1);
    let second = root.join("second");
    opened.save_snapshot(&second).expect("second");
    assert_eq!(opened.counters().component_loads, 1);
    let manifest = super::read_manifest(&second).expect("manifest");
    assert_eq!(
        manifest.components.len(),
        opened.component_descriptors().count()
    );
    assert!(second.join("manifest.json").is_file());
    let mut reopened = StablePrototype::open_snapshot(&second).expect("reopen");
    assert_eq!(
        reopened
            .file_by_id(FileId(0))
            .expect("file")
            .expect("present")
            .relative_path,
        fixture.repository.files().files[0].relative_path
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn failed_snapshot_publication_preserves_the_previous_generation() {
    let fixture = fixture("snapshot-failure");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("snapshot-failure-generations");
    let first = root.join("first");
    prototype.save_snapshot(&first).expect("first");
    let manifest_before = std::fs::read(first.join("manifest.json")).expect("manifest");
    assert!(prototype.save_snapshot(&first).is_err());
    let manifest_after = std::fs::read(first.join("manifest.json")).expect("manifest");
    assert_eq!(manifest_before, manifest_after);
    assert!(
        StablePrototype::open_snapshot(&first)
            .expect("previous generation remains usable")
            .file_by_id(FileId(0))
            .expect("file")
            .is_some()
    );
    let staging = std::fs::read_dir(&root)
        .expect("root")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".objekte-icss-staging-")
        })
        .count();
    assert_eq!(staging, 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn generation_reopen_supports_known_key_mutation_and_second_save() {
    let fixture = fixture("reopen-mutation");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("reopen-mutation-generations");
    let first = root.join("first");
    prototype.save_snapshot(&first).expect("first");
    drop(prototype);
    let mut reopened = StablePrototype::open_snapshot(&first).expect("open");
    let field = super::FieldComponentKey {
        path: vec!["age".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    let by_one = ScalarKey::Number(Number::integer(1));
    let before = reopened
        .typed_posting(CollectionId(0), &field, &by_one)
        .expect("before")
        .complete()
        .expect("complete before");
    let live = reopened.live_slots(CollectionId(0)).expect("live");
    let candidate = (&live - &before)
        .iter()
        .next()
        .expect("a live slot with another value");
    let add = RoaringBitmap::from_iter([candidate]);
    reopened
        .update_typed_posting(
            CollectionId(0),
            &field,
            &by_one,
            &add,
            &RoaringBitmap::new(),
        )
        .expect("add");
    let after = reopened
        .typed_posting(CollectionId(0), &field, &by_one)
        .expect("after")
        .complete()
        .expect("complete after");
    assert_eq!(after.len(), before.len() + 1);
    reopened
        .update_typed_posting(
            CollectionId(0),
            &field,
            &by_one,
            &RoaringBitmap::new(),
            &add,
        )
        .expect("remove");
    let second = root.join("second");
    reopened.save_snapshot(&second).expect("second save");
    drop(reopened);
    let mut second_open = StablePrototype::open_snapshot(&second).expect("reopen second");
    assert_eq!(
        second_open
            .typed_posting(CollectionId(0), &field, &by_one)
            .expect("posting")
            .complete()
            .expect("complete posting")
            .len(),
        before.len()
    );
    // The first generation remains unchanged after the later mutation/save.
    let mut first_open = StablePrototype::open_snapshot(&first).expect("reopen first");
    assert_eq!(
        first_open
            .typed_posting(CollectionId(0), &field, &by_one)
            .expect("first posting")
            .complete()
            .expect("complete first posting")
            .len(),
        before.len()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn empty_repository_components_open_and_save() {
    let root = temp_root("empty");
    std::fs::create_dir_all(root.join("data")).expect("data");
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    let repository = Repository::open(&root, vec![definition]).expect("empty repository");
    let mut prototype = StablePrototype::from_repository(&repository).expect("prototype");
    let report = prototype
        .validate_against_repository(&repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);
    assert_eq!(report.records, 0);
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    let mut opened = StablePrototype::open_snapshot(&generation).expect("open");
    assert_eq!(opened.live_slots(CollectionId(0)).expect("live").len(), 0);
    assert!(
        opened
            .records_for_identity(CollectionId(0), &RecordId("none".into()))
            .expect("identity")
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn multiple_collections_share_one_physical_path_catalog() {
    let root = temp_root("multi-collection");
    std::fs::create_dir_all(root.join("data")).expect("data");
    std::fs::write(
        root.join("data/shared.jsonl"),
        b"{\"name\":\"alpha\"}\n{\"name\":\"beta\"}\n",
    )
    .expect("write");
    let mut first = CollectionDefinition::folder("first", "data", SourceFormat::JsonLines);
    first
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("field");
    let mut second = CollectionDefinition::folder("second", "data", SourceFormat::JsonLines);
    second
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("field");
    let repository = Repository::open(&root, vec![first, second]).expect("repository");
    assert_eq!(repository.files().files.len(), 1);
    assert_eq!(repository.files().files[0].collections.len(), 2);
    let mut prototype = StablePrototype::from_repository(&repository).expect("prototype");
    let report = prototype
        .validate_against_repository(&repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);
    let path = repository.files().files[0].relative_path.clone();
    let refs = prototype.records_for_path(&path).expect("refs");
    assert_eq!(refs.len(), 4);
    assert_eq!(
        refs.iter()
            .filter(|reference| reference.collection == CollectionId(0))
            .count(),
        2
    );
    assert_eq!(
        refs.iter()
            .filter(|reference| reference.collection == CollectionId(1))
            .count(),
        2
    );
    let mut opened = StablePrototype::open_snapshot({
        let generation = root.join("gen");
        prototype.save_snapshot(&generation).expect("save");
        generation
    })
    .expect("open");
    assert_eq!(opened.records_for_path(&path).expect("open refs"), refs);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn relation_occurrences_targets_and_states_match_native() {
    let root = temp_root("relation");
    std::fs::create_dir_all(root.join("people")).expect("data");
    std::fs::write(
        root.join("people/a.json"),
        b"{\"id\":\"a\",\"name\":\"A\",\"friend\":\"b\"}",
    )
    .expect("a");
    std::fs::write(
        root.join("people/b.json"),
        b"{\"id\":\"b\",\"name\":\"B\",\"friend\":\"a\"}",
    )
    .expect("b");
    std::fs::write(
        root.join("people/c.json"),
        b"{\"id\":\"c\",\"name\":\"C\",\"friend\":\"missing\"}",
    )
    .expect("c");
    let mut definition = CollectionDefinition::folder("people", "people", SourceFormat::Json);
    definition.identity = crate::repository::IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition
        .schema
        .register_field(field(&["id"], LogicalType::Id, false, true))
        .expect("id");
    definition
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("name");
    definition
        .schema
        .register_field(field(&["friend"], LogicalType::Id, false, true))
        .expect("friend");
    definition
        .schema
        .register_relation(crate::relation::RelationDefinition {
            path: vec!["friend".into()],
            target_collection: "people".into(),
            cardinality: crate::relation::RelationCardinality::One,
            reverse_alias: None,
        })
        .expect("relation");
    let repository = Repository::open(&root, vec![definition]).expect("repository");
    let mut prototype = StablePrototype::from_repository(&repository).expect("prototype");
    let report = prototype
        .validate_against_repository(&repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);
    assert!(report.relation_occurrences >= 3);
    let descriptor = prototype
        .component_descriptors()
        .find(|descriptor| descriptor.kind == ComponentKind::RelationOccurrences)
        .expect("relation component")
        .clone();
    let key = super::RelationComponentKey {
        path: vec!["friend".into()],
        target_collection: "people".into(),
    };
    let occurrences = prototype
        .relation_occurrences(CollectionId(0), &key, RecordSlot(0))
        .expect("occurrences");
    assert_eq!(occurrences.len(), 1);
    assert_eq!(occurrences[0].target_id, "b");
    assert!(occurrences[0].target_resolved);
    let sources = prototype
        .relation_target_sources(CollectionId(0), &key, CollectionId(0), "b")
        .expect("target sources");
    assert!(sources.contains(0));
    let unavailable = prototype
        .relation_target_sources(CollectionId(0), &key, CollectionId(0), "missing")
        .expect("dangling target");
    assert!(unavailable.contains(2));
    assert_eq!(
        prototype
            .relation_present(CollectionId(0), &key)
            .expect("present")
            .len(),
        3
    );
    let _ = descriptor;
    let _ = std::fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_round_trip_through_component_keys() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(OsString::from_vec(vec![b'n', b'o', 0xFF, b'n', b'e']));
    let key = PathKey::from_path(&path).expect("path key");
    assert_eq!(key.to_path().expect("path"), path);
    let bytes = bincode::serialize(&ComponentKey::Path(key.clone())).expect("serialize");
    let decoded: ComponentKey = bincode::deserialize(&bytes).expect("deserialize");
    assert_eq!(decoded, ComponentKey::Path(key));
}

#[test]
fn move_updates_catalog_and_path_lookup_for_generated_identity() {
    let fixture = fixture("move");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let old_path = fixture.repository.files().files[1].relative_path.clone();
    let file = fixture.repository.files().files[1].id;
    let new_path = PathBuf::from("data/nested/renamed-c.jsonl");
    prototype.move_file(file, &new_path).expect("move");
    assert!(
        prototype
            .records_for_path(&old_path)
            .expect("old")
            .is_empty()
    );
    let refs = prototype.records_for_path(&new_path).expect("new");
    assert_eq!(refs.len(), 2);
    let entry = prototype.file_by_id(file).expect("file").expect("present");
    assert_eq!(entry.relative_path, new_path);
    let root = temp_root("move-generation");
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    drop(prototype);
    let mut reopened = StablePrototype::open_snapshot(&generation).expect("open");
    assert_eq!(reopened.records_for_path(&new_path).expect("refs").len(), 2);
    assert_eq!(
        reopened
            .file_by_id(file)
            .expect("file")
            .expect("present")
            .relative_path,
        new_path
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn direct_source_build_matches_native_semantics() {
    let root = temp_root("direct");
    std::fs::create_dir_all(root.join("data")).expect("data");
    std::fs::write(
        root.join("data/a.jsonl"),
        b"{\"name\":\"alpha\",\"age\":1,\"tags\":[\"x\",\"y\"]}\n{\"name\":\"beta\",\"age\":2,\"tags\":[\"x\"]}\n",
    )
    .expect("a");
    std::fs::write(
        root.join("data/b.jsonl"),
        b"{\"name\":\"alpha\",\"age\":10,\"tags\":[]}\n",
    )
    .expect("b");
    let definitions = vec![{
        let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
        definition
            .schema
            .register_field(field(&["name"], LogicalType::String, false, true))
            .expect("name");
        definition
            .schema
            .register_field(field(&["age"], LogicalType::Number, false, true))
            .expect("age");
        definition
            .schema
            .register_field(field(
                &["tags"],
                LogicalType::Array(Box::new(LogicalType::String)),
                true,
                true,
            ))
            .expect("tags");
        definition.sort_fields = vec![crate::model::FieldPath::parse("age").expect("sort")];
        definition
    }];
    let repository = Repository::open(&root, definitions.clone()).expect("native");
    let (mut direct, timings) = StablePrototype::from_sources_with_timings(
        &root,
        definitions,
        RepositoryOptions::default(),
    )
    .expect("direct");
    assert_eq!(timings.source_files, 2);
    assert_eq!(timings.source_records, 3);
    let report = direct
        .validate_against_repository(&repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);
    assert!(!direct.build_stage_ns().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn injected_snapshot_failures_preserve_usable_ram_and_previous_generation() {
    let _fault_guard = super::snapshot::fault_test_lock();
    let fixture = fixture("faults");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("faults-generations");
    let previous = root.join("previous");
    prototype.save_snapshot(&previous).expect("previous");
    let previous_manifest = std::fs::read(previous.join("manifest.json")).expect("manifest");
    for (index, point) in [
        "component-before-open",
        "component-after-open",
        "component-after-payload",
        "component-before-sync",
        "snapshot-before-manifest",
        "snapshot-before-sync",
        "snapshot-before-rename",
    ]
    .into_iter()
    .enumerate()
    {
        let destination = root.join(format!("failed-{index}"));
        super::snapshot::arm_fault(point, format!("failed-{index}"));
        let error = prototype
            .save_snapshot(&destination)
            .expect_err("fault must fail the save");
        assert!(error.to_string().contains("injected fault at"), "{error}");
        super::snapshot::clear_fault();
        assert!(!destination.exists());
        assert!(
            prototype
                .file_by_id(FileId(0))
                .expect("ram remains usable")
                .is_some()
        );
        assert_eq!(
            std::fs::read(previous.join("manifest.json")).expect("manifest"),
            previous_manifest
        );
        let recovered = root.join(format!("recovered-{index}"));
        prototype
            .save_snapshot(&recovered)
            .expect("retry after fault");
        assert!(recovered.join("manifest.json").is_file());
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn malformed_manifest_missing_member_and_swapped_component_fail_explicitly() {
    let fixture = fixture("manifest-corruption");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("manifest-corruption-generations");
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    let manifest_path = generation.join("manifest.json");
    let original = std::fs::read(&manifest_path).expect("manifest");

    std::fs::write(&manifest_path, b"{not json").expect("malformed");
    assert!(StablePrototype::open_snapshot(&generation).is_err());
    std::fs::write(&manifest_path, &original).expect("restore");

    let manifest: serde_json::Value = serde_json::from_slice(&original).expect("json");
    let first_path = manifest["components"][0]["path"]
        .as_str()
        .expect("path")
        .to_string();
    let member = generation.join(&first_path);
    let saved = std::fs::read(&member).expect("member");
    std::fs::remove_file(&member).expect("remove member");
    assert!(StablePrototype::open_snapshot(&generation).is_err());
    std::fs::write(&member, &saved).expect("restore member");

    let second_path = manifest["components"][1]["path"]
        .as_str()
        .expect("path")
        .to_string();
    let second = std::fs::read(generation.join(&second_path)).expect("second");
    std::fs::write(&member, &second).expect("swap component");
    assert!(StablePrototype::open_snapshot(&generation).is_err());
    std::fs::write(&member, &saved).expect("restore member");
    assert!(StablePrototype::open_snapshot(&generation).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn exact_numbers_null_states_and_custom_text_match_native() {
    let root = temp_root("semantic-families");
    std::fs::create_dir_all(root.join("data")).expect("data");
    std::fs::write(
        root.join("data/numbers.jsonl"),
        br#"{"n":-5}
{"n":-1}
{"n":0}
{"n":1}
{"n":10}
{"n":1000000000000000000000000000000}
{"n":0.000000000000000000000000000001}
{"n":1.5}
{}
{"n":null}
{"arr":[]}
{"arr":[null]}
{"arr":["a"]}
{"arr":["b","c"]}
"#,
    )
    .expect("numbers");
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(field(&["n"], LogicalType::Number, false, true))
        .expect("n");
    definition
        .schema
        .register_field(field(
            &["arr"],
            LogicalType::Array(Box::new(LogicalType::String)),
            true,
            true,
        ))
        .expect("arr");
    definition
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("name");
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
        .expect("custom");
    let repository = Repository::open(&root, vec![definition.clone()]).expect("native");
    let (mut direct, _) = StablePrototype::from_sources_with_timings(
        &root,
        vec![definition],
        RepositoryOptions::default(),
    )
    .expect("direct");
    let report = direct
        .validate_against_repository(&repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);

    let n = super::FieldComponentKey {
        path: vec!["n".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    let arr = super::FieldComponentKey {
        path: vec!["arr".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    assert_eq!(
        direct
            .state_posting(CollectionId(0), &n, super::STATE_PRESENCE)
            .expect("n presence")
            .complete()
            .expect("complete n presence")
            .len(),
        9
    );
    assert_eq!(
        direct
            .state_posting(CollectionId(0), &n, super::STATE_LEGACY_NULL)
            .expect("n null")
            .complete()
            .expect("complete n null")
            .len(),
        1
    );
    assert_eq!(
        direct
            .state_posting(CollectionId(0), &arr, super::STATE_PRESENCE)
            .expect("arr presence")
            .complete()
            .expect("complete arr presence")
            .len(),
        4
    );
    assert_eq!(
        direct
            .state_posting(CollectionId(0), &arr, super::STATE_ARRAY)
            .expect("arr arrays")
            .complete()
            .expect("complete arr arrays")
            .len(),
        4
    );
    let negative = direct
        .typed_range_slots(
            CollectionId(0),
            &n,
            std::ops::Bound::Unbounded,
            std::ops::Bound::Excluded(ScalarKey::Number(Number::integer(0))),
            false,
        )
        .expect("negative range")
        .complete()
        .expect("complete negative range");
    assert_eq!(negative.len(), 2);
    let custom = direct
        .component_descriptors()
        .find(|descriptor| {
            descriptor.kind == ComponentKind::NormalGrams
                && descriptor.field.as_ref().is_some_and(|field| {
                    field.path == vec!["name".to_string()]
                        && field.binding.as_deref() == Some("folded")
                })
        })
        .expect("custom text component");
    assert!(custom.field.as_ref().expect("field").role == super::BindingRole::CustomText);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn malformed_jsonl_raw_fallback_is_indexed_as_unavailable_state() {
    let root = temp_root("raw-fallback");
    std::fs::create_dir_all(root.join("data")).expect("data");
    std::fs::write(
        root.join("data/mixed.jsonl"),
        b"{\"name\":\"ok\"}\n{broken}\n{\"name\":\"also-ok\"}\n",
    )
    .expect("mixed");
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("name");
    let repository = Repository::open(&root, vec![definition.clone()]).expect("native");
    let (mut direct, _) = StablePrototype::from_sources_with_timings(
        &root,
        vec![definition],
        RepositoryOptions::default(),
    )
    .expect("direct");
    let report = direct
        .validate_against_repository(&repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);
    let raw = direct
        .domain_posting(CollectionId(0), super::DOMAIN_RAW_FALLBACK)
        .expect("raw domain");
    assert_eq!(raw.len(), 1);
    let slot = RecordSlot(raw.iter().next().expect("raw slot"));
    let record = direct
        .record_by_slot(CollectionId(0), slot)
        .expect("record")
        .expect("live raw fallback");
    assert_eq!(record.class, crate::inventory::RecordClass::RawFallback);
    assert_ne!(record.diagnostic_ref, crate::inventory::DiagnosticRef(0));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unicode_alphabet_direct_build_matches_native() {
    let root = temp_root("unicode");
    std::fs::create_dir_all(root.join("data")).expect("data");
    std::fs::write(
        root.join("data/unicode.jsonl"),
        "{\"name\":\"Ärger\",\"text\":\"Straße ÄÖÜ ß\"}\n{\"name\":\"arger\",\"text\":\"STRASSE ärger\"}\n"
            .as_bytes(),
    )
    .expect("unicode");
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("name");
    definition
        .schema
        .register_field(field(&["text"], LogicalType::String, false, true))
        .expect("text");
    let options = RepositoryOptions {
        bigram_alphabet: crate::workload::BigramAlphabet::Unicode,
        ..RepositoryOptions::default()
    };
    let repository =
        Repository::open_with_options(&root, vec![definition.clone()], options.clone())
            .expect("native");
    let (mut direct, _) =
        StablePrototype::from_sources_with_timings(&root, vec![definition], options)
            .expect("direct");
    let report = direct
        .validate_against_repository(&repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn worker_counts_do_not_change_descriptors_or_semantics() {
    let fixture = fixture("worker-determinism");
    let definitions = vec![fixture.repository.collections()[0].definition.clone()];
    let mut signatures = Vec::new();
    for workers in [1usize, 2, 4] {
        let (mut prototype, _) = StablePrototype::from_sources_with_timings(
            &fixture.root,
            definitions.clone(),
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(workers),
                ..RepositoryOptions::default()
            },
        )
        .expect("direct");
        let report = prototype
            .validate_against_repository(&fixture.repository)
            .expect("validation");
        assert!(report.is_valid(), "{workers}: {:?}", report.errors);
        let mut descriptors = prototype
            .component_descriptors()
            .map(|descriptor| {
                (
                    descriptor.id,
                    descriptor.kind,
                    descriptor.sort_key(),
                    descriptor.coverage.clone(),
                )
            })
            .collect::<Vec<_>>();
        descriptors.sort_by_key(|entry| entry.0);
        signatures.push(descriptors);
    }
    assert_eq!(signatures[0], signatures[1]);
    assert_eq!(signatures[0], signatures[2]);
}

#[test]
fn all_component_load_uses_no_insertion_calls() {
    let fixture = fixture("all-component-load");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("all-component-load-generation");
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    drop(prototype);
    let mut opened = StablePrototype::open_snapshot(&generation).expect("open");
    let ids = opened
        .component_descriptors()
        .map(|descriptor| descriptor.id)
        .collect::<Vec<_>>();
    for id in &ids {
        opened.load_component(*id).expect("load component");
    }
    assert_eq!(opened.counters().component_inserts_during_load, 0);
    assert_eq!(opened.counters().component_loads, ids.len() as u64);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn snapshot_catalogs_and_postings_work_without_source_files() {
    let fixture = fixture("source-free");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let root = temp_root("source-free-generation");
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    for file in &fixture.repository.files().files {
        let _ = std::fs::remove_file(fixture.root.join(&file.relative_path));
    }
    let mut opened = StablePrototype::open_snapshot(&generation).expect("open");
    assert!(opened.file_by_id(FileId(0)).expect("file").is_some());
    let path = fixture.repository.files().files[0].relative_path.clone();
    assert_eq!(opened.records_for_path(&path).expect("refs").len(), 2);
    let age = super::FieldComponentKey {
        path: vec!["age".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    assert_eq!(
        opened
            .typed_posting(
                CollectionId(0),
                &age,
                &ScalarKey::Number(Number::integer(1))
            )
            .expect("posting")
            .complete()
            .expect("complete posting")
            .len(),
        2
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn component_descriptors_record_catalog_dependencies() {
    let fixture = fixture("dependencies");
    let prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let domains = prototype
        .component_descriptors()
        .find(|descriptor| {
            descriptor.kind == ComponentKind::Domains
                && descriptor.collection.as_ref().map(|value| value.id) == Some(CollectionId(0))
        })
        .expect("domains")
        .id;
    let records = prototype
        .component_descriptors()
        .find(|descriptor| {
            descriptor.kind == ComponentKind::Records
                && descriptor.collection.as_ref().map(|value| value.id) == Some(CollectionId(0))
        })
        .expect("records")
        .id;
    let value = prototype
        .component_descriptors()
        .find(|descriptor| {
            descriptor.kind == ComponentKind::Values
                && descriptor.collection.as_ref().map(|value| value.id) == Some(CollectionId(0))
        })
        .expect("values");
    assert!(value.dependency_ids.contains(&domains));
    assert!(value.dependency_ids.contains(&records));
    let domains_descriptor = prototype
        .component_descriptor(domains)
        .expect("domains descriptor");
    assert!(domains_descriptor.dependency_ids.is_empty());
}

#[test]
fn text_candidates_use_native_gram_preparation() {
    let fixture = fixture("text-candidates");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let key = super::TextComponentKey {
        path: Vec::new(),
        binding: None,
        role: super::BindingRole::RawText,
    };
    let candidates = prototype
        .text_candidates(
            CollectionId(0),
            &key,
            "alpha",
            crate::workload::BigramAlphabet::Ascii,
        )
        .expect("candidates");
    let candidates = candidates.expect("literal has usable grams");
    assert!(!candidates.is_empty());
    assert!(candidates.is_subset(&prototype.live_slots(CollectionId(0)).expect("live")));
}

fn prepared_facts_for_file(
    prototype: &StablePrototype,
    state: &crate::repository::CollectionState,
    file_id: FileId,
) -> (Vec<RecordEntry>, Vec<super::PreparedPostingAdd>) {
    let records = state
        .generation
        .records
        .iter()
        .filter(|record| record.file == file_id)
        .cloned()
        .collect::<Vec<_>>();
    let old_slots = RoaringBitmap::from_iter(records.iter().map(|record| record.slot.0));
    let local = records
        .iter()
        .enumerate()
        .map(|(index, record)| (record.slot.0, index as u32))
        .collect::<BTreeMap<_, _>>();
    let localize = |bitmap: &RoaringBitmap| {
        RoaringBitmap::from_iter((bitmap & &old_slots).iter().map(|slot| local[&slot]))
    };
    let mut adds = Vec::new();
    let collection = state.generation.collection;
    let mut add = |component, key, bitmap: &RoaringBitmap| {
        let slots = localize(bitmap);
        if !slots.is_empty() {
            adds.push(super::PreparedPostingAdd {
                component,
                key,
                slots,
                value: None,
            });
        }
    };
    for (path, index) in &state.postings.fields {
        let key = super::FieldComponentKey {
            path: path.clone(),
            binding: None,
            role: super::BindingRole::DeclaredField,
        };
        let ids = prototype
            .field_components(collection, &key)
            .expect("field components");
        if let Some(component) = ids.values {
            for (value, bitmap) in &index.values {
                add(
                    component,
                    super::ComponentKey::Scalar(value.clone()),
                    bitmap,
                );
            }
        }
        if let Some(component) = ids.lengths {
            for (value, bitmap) in &index.lengths {
                add(
                    component,
                    super::ComponentKey::Length(value.clone()),
                    bitmap,
                );
            }
        }
        if let Some(component) = ids.states {
            add(
                component,
                super::ComponentKey::State(super::STATE_PRESENCE),
                &index.presence,
            );
            add(
                component,
                super::ComponentKey::State(super::STATE_EMPTY_SELECTION),
                &index.empty_selections,
            );
            add(
                component,
                super::ComponentKey::State(super::STATE_LEGACY_NULL),
                &index.nulls,
            );
            if let Some(facts) = &index.structural {
                for (key, bitmap) in &facts.postings {
                    let state_code = match key {
                        crate::posting::StructuralKey::AnyNull => super::STATE_ANY_NULL,
                        crate::posting::StructuralKey::NonEmpty => super::STATE_NON_EMPTY,
                        crate::posting::StructuralKey::String => super::STATE_STRING,
                        crate::posting::StructuralKey::Number => super::STATE_NUMBER,
                        crate::posting::StructuralKey::Array => super::STATE_ARRAY,
                        crate::posting::StructuralKey::Object => super::STATE_OBJECT,
                        crate::posting::StructuralKey::EmptyObject => super::STATE_EMPTY_OBJECT,
                    };
                    add(component, super::ComponentKey::State(state_code), bitmap);
                }
            }
        }
    }
    for ((path, name), custom) in &state.custom_postings {
        let key = super::FieldComponentKey {
            path: path.clone(),
            binding: Some(name.clone()),
            role: super::BindingRole::NamedBinding,
        };
        let ids = prototype
            .field_components(collection, &key)
            .expect("custom field components");
        if let Some(component) = ids.values {
            for (value, bitmap) in &custom.index.values {
                add(
                    component,
                    super::ComponentKey::Scalar(value.clone()),
                    bitmap,
                );
            }
        }
    }
    for (key, index) in [
        (
            super::TextComponentKey {
                path: Vec::new(),
                binding: None,
                role: super::BindingRole::RawText,
            },
            &state.raw,
        ),
        (
            super::TextComponentKey {
                path: Vec::new(),
                binding: None,
                role: super::BindingRole::FallbackText,
            },
            &state.fallback_text,
        ),
    ] {
        let ids = prototype
            .text_components(collection, &key)
            .expect("text components");
        if let Some(component) = ids.normal {
            for (gram, bitmap) in &index.normal {
                add(component, super::ComponentKey::Gram(*gram), bitmap);
            }
        }
        if let Some(component) = ids.skip {
            for (gram, bitmap) in &index.skip {
                add(component, super::ComponentKey::Gram(*gram), bitmap);
            }
        }
    }
    for ((path, name), index) in &state.custom_text {
        let key = super::TextComponentKey {
            path: path.clone(),
            binding: Some(name.clone()),
            role: super::BindingRole::CustomText,
        };
        if let Some(ids) = prototype.text_components(collection, &key) {
            if let Some(component) = ids.normal {
                for (gram, bitmap) in &index.normal {
                    add(component, super::ComponentKey::Gram(*gram), bitmap);
                }
            }
            if let Some(component) = ids.skip {
                for (gram, bitmap) in &index.skip {
                    add(component, super::ComponentKey::Gram(*gram), bitmap);
                }
            }
        }
    }
    for relation in &state.relations {
        let key = super::RelationComponentKey {
            path: relation.declaration.path.clone(),
            target_collection: relation.declaration.target_collection.clone(),
        };
        let Some(ids) = prototype.relation_components(collection, &key) else {
            continue;
        };
        let mut targets = BTreeMap::<String, RoaringBitmap>::new();
        for record in &records {
            let source = record.slot;
            let Some(edges) = relation.edges(source) else {
                continue;
            };
            let local_slot = local[&source.0];
            for (ordinal, edge) in edges.iter().enumerate() {
                let value = codec::encode_meta(&super::RelationOccurrenceValue {
                    target_collection: Some(relation.target_collection),
                    target_id: edge.target_id().to_owned(),
                    positions: edge.positions.clone(),
                    target_resolved: matches!(
                        edge.target,
                        crate::relation::RelationTarget::Resolved(_)
                    ),
                })
                .expect("relation occurrence metadata");
                adds.push(super::PreparedPostingAdd {
                    component: ids.occurrences,
                    key: super::ComponentKey::Occurrence(super::codec::RelationOccurrenceKey {
                        source: RecordSlot(local_slot),
                        ordinal: ordinal as u32,
                    }),
                    slots: RoaringBitmap::new(),
                    value: Some(value),
                });
                targets
                    .entry(edge.target_id().to_owned())
                    .or_default()
                    .insert(local_slot);
            }
        }
        for (target_id, sources) in targets {
            adds.push(super::PreparedPostingAdd {
                component: ids.targets,
                key: super::ComponentKey::RelationTarget(super::codec::RelationTargetKey {
                    target_collection: relation.target_collection,
                    target_id,
                }),
                slots: sources,
                value: None,
            });
        }
        for (state_code, bitmap) in [
            (super::codec::RELATION_PRESENT, relation.present()),
            (super::codec::RELATION_NULLS, relation.nulls()),
            (super::codec::RELATION_EMPTY_ARRAY, relation.empty_arrays()),
            (super::codec::RELATION_UNAVAILABLE, relation.unavailable()),
        ] {
            adds.push(super::PreparedPostingAdd {
                component: ids.states,
                key: super::ComponentKey::RelationState(state_code),
                slots: localize(bitmap),
                value: None,
            });
        }
    }
    for descriptor in prototype.components.values() {
        let Some(collection_identity) = descriptor.descriptor.collection.as_ref() else {
            continue;
        };
        if collection_identity.id != collection
            || !matches!(
                descriptor.descriptor.kind,
                super::ComponentKind::SortAscending | super::ComponentKind::SortDescending
            )
        {
            continue;
        }
        let Some(field) = descriptor.descriptor.field.as_ref() else {
            continue;
        };
        let Some(ordering) = descriptor.descriptor.ordering.as_ref() else {
            continue;
        };
        let Some(column) = state
            .sorting
            .columns
            .iter()
            .find(|(path, _)| path.segments == field.path)
            .map(|(_, column)| column)
        else {
            continue;
        };
        let descending = descriptor.descriptor.kind == super::ComponentKind::SortDescending;
        let component = descriptor.descriptor.id;
        for (index, record) in records.iter().enumerate() {
            adds.push(super::PreparedPostingAdd {
                component,
                key: super::ComponentKey::Sort(SortEntryKey {
                    options: ordering.options,
                    descending,
                    key: column.keys[record.slot.0 as usize].clone(),
                    tie_break_id: record.id.clone(),
                    slot: RecordSlot(index as u32),
                }),
                slots: RoaringBitmap::new(),
                value: Some(StoredBytes::empty()),
            });
        }
    }
    (records, adds)
}

#[test]
fn prepared_replacement_preserves_all_indexed_facts() {
    let fixture = fixture("prepared-replacement");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let state = &fixture.repository.collections()[0];
    let file = fixture.repository.files().files[0].clone();
    let path = file.relative_path.to_path_buf();
    let (records, posting_adds) = prepared_facts_for_file(&prototype, state, file.id);
    let age = super::FieldComponentKey {
        path: vec!["age".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    let before_identity = prototype
        .records_for_identity(CollectionId(0), &records[0].id)
        .expect("identity before");
    prototype
        .replace_prepared_file(
            &path,
            super::PreparedFileChange {
                file,
                collections: vec![super::PreparedCollectionChange {
                    collection: CollectionId(0),
                    records: records.clone(),
                    posting_adds,
                    coverage: Vec::new(),
                }],
            },
        )
        .expect("prepared replacement");
    let one = prototype
        .typed_posting(
            CollectionId(0),
            &age,
            &ScalarKey::Number(Number::integer(1)),
        )
        .expect("typed")
        .complete()
        .expect("complete typed");
    assert_eq!(one.len(), 2);
    let after_identity = prototype
        .records_for_identity(CollectionId(0), &records[0].id)
        .expect("identity after");
    assert_ne!(before_identity, after_identity);
    assert_eq!(after_identity.len(), 1);
    assert_eq!(prototype.records_for_path(&path).expect("path").len(), 2);
    let sort_descriptor = prototype
        .component_descriptors()
        .find(|descriptor| descriptor.kind == ComponentKind::SortAscending)
        .expect("sort")
        .clone();
    let sort_key = super::sort_component_key(&sort_descriptor);
    let sorted = prototype
        .sort_slots(CollectionId(0), &sort_key, false)
        .expect("sorted");
    let ids = sorted
        .iter()
        .filter_map(|slot| {
            prototype
                .record_by_slot(CollectionId(0), *slot)
                .ok()
                .flatten()
                .map(|record| record.id)
        })
        .collect::<Vec<_>>();
    let mut expected = state
        .sorting
        .columns
        .values()
        .next()
        .expect("native sort")
        .ascending
        .iter()
        .map(|slot| state.generation.records[slot.0 as usize].id.clone())
        .collect::<Vec<_>>();
    expected.sort_by(|left, right| left.0.len().cmp(&right.0.len()).then(left.cmp(right)));
    let mut actual = ids.clone();
    actual.sort_by(|left, right| left.0.len().cmp(&right.0.len()).then(left.cmp(right)));
    assert_eq!(actual, expected);
    let root = temp_root("prepared-replacement-gen");
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    drop(prototype);
    let mut reopened = StablePrototype::open_snapshot(&generation).expect("open");
    assert_eq!(
        reopened
            .typed_posting(
                CollectionId(0),
                &age,
                &ScalarKey::Number(Number::integer(1))
            )
            .expect("typed")
            .complete()
            .expect("complete")
            .len(),
        2
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn prepared_replacement_changed_facts_match_native_rebuild() {
    let fixture = fixture("prepared-changed");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let definitions = vec![fixture.repository.collections()[0].definition.clone()];
    std::fs::write(
        fixture.root.join("data/a.jsonl"),
        b"{\"name\":\"omega\",\"age\":77,\"tags\":[\"q\",\"r\",\"s\"]}\n{\"name\":\"beta\",\"age\":2,\"tags\":[]}\n",
    )
    .expect("changed source");
    let changed = Repository::open(&fixture.root, definitions.clone()).expect("native rebuild");
    let state = &changed.collections()[0];
    let file = fixture.repository.files().files[0].clone();
    let path = file.relative_path.to_path_buf();
    let (records, posting_adds) = prepared_facts_for_file(&prototype, state, file.id);
    prototype
        .replace_prepared_file(
            &path,
            super::PreparedFileChange {
                file,
                collections: vec![super::PreparedCollectionChange {
                    collection: CollectionId(0),
                    records,
                    posting_adds,
                    coverage: Vec::new(),
                }],
            },
        )
        .expect("prepared replacement");
    let age = super::FieldComponentKey {
        path: vec!["age".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    let native_age = state.postings.fields[&vec!["age".to_string()]]
        .values
        .get(&ScalarKey::Number(Number::integer(77)))
        .expect("native age")
        .len();
    assert_eq!(
        prototype
            .typed_posting(
                CollectionId(0),
                &age,
                &ScalarKey::Number(Number::integer(77))
            )
            .expect("stable age")
            .complete()
            .expect("complete age")
            .len(),
        native_age as u64
    );
    let text_key = super::TextComponentKey {
        path: Vec::new(),
        binding: None,
        role: super::BindingRole::RawText,
    };
    let candidates = prototype
        .text_candidates(
            CollectionId(0),
            &text_key,
            "omega",
            crate::workload::BigramAlphabet::Ascii,
        )
        .expect("candidates")
        .expect("grams");
    assert!(!candidates.is_empty());
    let record_ids = |prototype: &mut StablePrototype| {
        let descriptor = prototype
            .component_descriptors()
            .find(|descriptor| descriptor.kind == ComponentKind::SortAscending)
            .expect("sort")
            .clone();
        let key = super::sort_component_key(&descriptor);
        prototype
            .sort_slots(CollectionId(0), &key, false)
            .expect("sort slots")
            .into_iter()
            .filter_map(|slot| {
                prototype
                    .record_by_slot(CollectionId(0), slot)
                    .ok()
                    .flatten()
                    .map(|record| record.id)
            })
            .collect::<Vec<_>>()
    };
    let actual = record_ids(&mut prototype);
    let mut expected = state
        .sorting
        .columns
        .values()
        .next()
        .expect("native sort")
        .ascending
        .iter()
        .map(|slot| state.generation.records[slot.0 as usize].id.clone())
        .collect::<Vec<_>>();
    assert_eq!(actual.len(), expected.len());
    expected.sort_by(|left, right| left.0.len().cmp(&right.0.len()).then(left.cmp(right)));
    let mut actual_sorted = actual.clone();
    actual_sorted.sort_by(|left, right| left.0.len().cmp(&right.0.len()).then(left.cmp(right)));
    assert_eq!(actual_sorted, expected);
    let root = temp_root("prepared-changed-gen");
    let generation = root.join("gen");
    prototype.save_snapshot(&generation).expect("save");
    drop(prototype);
    let mut reopened = StablePrototype::open_snapshot(&generation).expect("open");
    assert_eq!(
        reopened
            .typed_posting(
                CollectionId(0),
                &age,
                &ScalarKey::Number(Number::integer(77))
            )
            .expect("typed")
            .complete()
            .expect("complete")
            .len(),
        native_age as u64
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn coverage_contract_distinguishes_no_restriction_from_complete_empty() {
    let root = temp_root("coverage-contract");
    std::fs::create_dir_all(root.join("data")).expect("data");
    std::fs::write(root.join("data/a.jsonl"), b"{\"name\":\"alpha\"}\n").expect("source");
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("name");
    definition
        .schema
        .register_field(field(&["note"], LogicalType::String, false, false))
        .expect("note");
    let repository = Repository::open(&root, vec![definition]).expect("native");
    let mut prototype = StablePrototype::from_repository(&repository).expect("prototype");
    let name = super::FieldComponentKey {
        path: vec!["name".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    let note = super::FieldComponentKey {
        path: vec!["note".into()],
        binding: None,
        role: super::BindingRole::DeclaredField,
    };
    assert!(matches!(
        prototype
            .typed_posting(
                CollectionId(0),
                &name,
                &ScalarKey::String("absent".into())
            )
            .expect("complete lookup"),
        super::Restriction::Complete(ref bitmap) if bitmap.is_empty()
    ));
    assert!(matches!(
        prototype
            .typed_posting(CollectionId(0), &note, &ScalarKey::String("absent".into()))
            .expect("incomplete lookup"),
        super::Restriction::NoRestriction
    ));
    assert!(matches!(
        prototype
            .length_posting(CollectionId(0), &name, &Number::integer(5))
            .expect("length lookup"),
        super::Restriction::NoRestriction
    ));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn failure_after_first_change_blocks_queries_and_saves() {
    let _fault_guard = super::snapshot::fault_test_lock();
    let fixture = fixture("mutation-failure");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let state = &fixture.repository.collections()[0];
    let file = fixture.repository.files().files[0].clone();
    let path = file.relative_path.to_path_buf();
    let (records, posting_adds) = prepared_facts_for_file(&prototype, state, file.id);
    super::snapshot::arm_fault("mutation-after-first-change", "data/a.jsonl");
    let result = prototype.replace_prepared_file(
        &path,
        super::PreparedFileChange {
            file,
            collections: vec![super::PreparedCollectionChange {
                collection: CollectionId(0),
                records,
                posting_adds,
                coverage: Vec::new(),
            }],
        },
    );
    super::snapshot::clear_fault();
    assert!(result.is_err());
    assert!(prototype.unusable_reason().is_some());
    assert!(matches!(
        prototype.file_by_id(FileId(0)),
        Err(StableIndexError::InvalidState(_))
    ));
    let generation = temp_root("mutation-failure-gen").join("gen");
    assert!(prototype.save_snapshot(&generation).is_err());
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn zero_record_paths_delete_and_collide_correctly() {
    let fixture = fixture("zero-record");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let mut file = fixture.repository.files().files[0].clone();
    file.id = prototype.next_file_id().expect("next file");
    file.relative_path = "data/empty.jsonl".into();
    file.collections = vec![CollectionId(0)];
    let empty_path = PathBuf::from("data/empty.jsonl");
    prototype
        .append_prepared_file(super::PreparedFileChange {
            file,
            collections: vec![super::PreparedCollectionChange {
                collection: CollectionId(0),
                records: Vec::new(),
                posting_adds: Vec::new(),
                coverage: Vec::new(),
            }],
        })
        .expect("append empty file");
    let entry = prototype
        .path_entry(&PathKey::from_path(&empty_path).expect("key"))
        .expect("path entry")
        .expect("present");
    assert!(entry.refs.is_empty());
    let existing_path = fixture.repository.files().files[1].relative_path.clone();
    assert!(
        prototype
            .move_file(fixture.repository.files().files[1].id, &empty_path)
            .is_err()
    );
    assert!(
        prototype
            .move_file(fixture.repository.files().files[1].id, &existing_path)
            .is_ok()
    );
    let deleted = prototype
        .delete_file_by_path(&empty_path)
        .expect("delete empty");
    assert!(deleted.is_empty());
    assert!(
        prototype
            .path_entry(&PathKey::from_path(&empty_path).expect("key"))
            .expect("lookup")
            .is_none()
    );
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn atomic_standalone_failure_preserves_ram_and_previous_files() {
    let _fault_guard = super::snapshot::fault_test_lock();
    let fixture = fixture("atomic-standalone");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let age = age_component(&prototype);
    let root = temp_root("atomic-standalone-files");
    let first = root.join("values.icss");
    let info = prototype.save_index(age, &first).expect("save first");
    assert!(first.is_file());
    let second = root.join("failed.icss");
    super::snapshot::arm_fault("standalone-before-rename", "failed.icss");
    assert!(prototype.save_index(age, &second).is_err());
    super::snapshot::clear_fault();
    assert!(!second.exists());
    assert!(first.is_file());
    assert_eq!(
        prototype
            .component_statistics(age)
            .expect("statistics")
            .entry_count,
        info.entry_count
    );
    prototype.save_index(age, &second).expect("retry");
    assert!(second.is_file());
    let mut standalone = StablePrototype::open_index(&first).expect("open standalone");
    let failed = root.join("failed-save-as.icss");
    super::snapshot::arm_fault("standalone-before-rename", "failed-save-as.icss");
    assert!(standalone.save_as(&failed).is_err());
    super::snapshot::clear_fault();
    assert!(!failed.exists());
    standalone.save_as(&failed).expect("retry save as");
    assert!(failed.is_file());
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn failed_snapshot_does_not_commit_dirty_or_saved_metadata() {
    let _fault_guard = super::snapshot::fault_test_lock();
    let fixture = fixture("snapshot-metadata");
    let mut prototype = StablePrototype::from_repository(&fixture.repository).expect("prototype");
    let age = age_component(&prototype);
    let before = prototype.component_statistics(age).expect("before");
    assert!(before.dirty);
    let root = temp_root("snapshot-metadata-gen");
    let generation = root.join("gen");
    super::snapshot::arm_fault(
        "snapshot-before-rename",
        generation.to_string_lossy().to_string(),
    );
    assert!(prototype.save_snapshot(&generation).is_err());
    super::snapshot::clear_fault();
    let after = prototype.component_statistics(age).expect("after");
    assert!(after.dirty);
    assert!(after.path.is_none());
    assert!(!generation.exists());
    prototype.save_snapshot(&generation).expect("retry");
    assert!(
        !prototype
            .component_statistics(age)
            .expect("committed")
            .dirty
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn configured_composite_sort_matches_native_order() {
    let root = temp_root("composite-sort");
    std::fs::create_dir_all(root.join("data")).expect("data");
    std::fs::write(
        root.join("data/a.jsonl"),
        b"{\"name\":\"beta\",\"age\":2}\n{\"name\":\"alpha\",\"age\":1}\n{\"name\":\"beta\",\"age\":1}\n",
    )
    .expect("source");
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::JsonLines);
    definition
        .schema
        .register_field(field(&["name"], LogicalType::String, false, true))
        .expect("name");
    definition
        .schema
        .register_field(field(&["age"], LogicalType::Number, false, true))
        .expect("age");
    definition.sort_fields = vec![
        crate::model::FieldPath::parse("age").expect("age sort"),
        crate::model::FieldPath::parse("name").expect("name sort"),
    ];
    definition
        .schema
        .register_composite_sort(vec![
            crate::workload::SortField::simple("age", false),
            crate::workload::SortField::simple("name", true),
        ])
        .expect("composite");
    let repository = Repository::open(&root, vec![definition.clone()]).expect("native");
    let mut prototype = StablePrototype::from_repository(&repository).expect("prototype");
    let report = prototype
        .validate_against_repository(&repository)
        .expect("validation");
    assert!(report.is_valid(), "{:?}", report.errors);
    let descriptor = prototype
        .component_descriptors()
        .find(|descriptor| descriptor.kind == ComponentKind::CompositeSort)
        .expect("composite component")
        .clone();
    let key = super::composite_component_key(&descriptor);
    let actual = prototype
        .sort_composite_slots(CollectionId(0), &key)
        .expect("composite slots");
    let native = &repository.collections()[0].sorting.composites[0].order;
    assert_eq!(&actual, native);
    let _ = std::fs::remove_dir_all(&root);
}
