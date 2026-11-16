//! Focused unit tests for the independent stable-index prototype.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
        .expect("one");
    assert_eq!(one.len(), 2);
    let range = prototype
        .typed_range_slots(
            CollectionId(0),
            &age,
            std::ops::Bound::Included(ScalarKey::Number(Number::integer(1))),
            std::ops::Bound::Excluded(ScalarKey::Number(Number::integer(10))),
            false,
        )
        .expect("range");
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
            slot: RecordSlot(slot),
        })
    };
    assert_eq!(
        sort_key(1).tag(),
        ComponentKey::Sort(SortEntryKey {
            options: sort_descriptor.ordering.as_ref().unwrap().options,
            descending: false,
            key: crate::sorting::SortKey::Unavailable,
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
        .expect("before");
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
        .expect("after");
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
            .len(),
        before.len()
    );
    // The first generation remains unchanged after the later mutation/save.
    let mut first_open = StablePrototype::open_snapshot(&first).expect("reopen first");
    assert_eq!(
        first_open
            .typed_posting(CollectionId(0), &field, &by_one)
            .expect("first posting")
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
