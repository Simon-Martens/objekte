use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::Arc;

use roaring::RoaringBitmap;
use time::{Date, Month, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::decimal::ExactDecimal;
use crate::model::Number;
use crate::posting::{
    PostingDictionary, PostingNumberKey, ScalarKey, StoredScalarKey,
};
use crate::lifecycle::{CompactionMode, CompactionOptions, Index, IndexView};
use crate::model::LogicalType;
use crate::query::{
    CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use crate::reference::FieldDefinition;
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions,
};
use crate::source::SourceFormat;
use crate::workload::{ReturnMode, SortField};

fn bitmap(slots: impl IntoIterator<Item = u32>) -> RoaringBitmap {
    slots.into_iter().collect()
}

fn prototype_definition() -> CollectionDefinition {
    let mut definition =
        CollectionDefinition::folder("items", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    for (name, kind) in [
        ("id", LogicalType::Id),
        ("year", LogicalType::Number),
        ("title", LogicalType::String),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: name == "id",
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    definition
}

fn prototype_options() -> RepositoryOptions {
    RepositoryOptions {
        segment_policy: crate::native_segment::SegmentPolicy {
            target_records: std::num::NonZeroUsize::new(2).unwrap(),
            max_files: std::num::NonZeroUsize::new(2).unwrap(),
        },
        ..Default::default()
    }
}

fn query(fts: Option<&str>, year_filter: Option<&str>, limit: usize, offset: usize) -> Query {
    Query {
        collections: vec!["items".into()],
        fts: fts.map(|literal| FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: literal.into(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        filter: year_filter.map(|filter| parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("year", false)],
            limit,
            offset,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn ids(view: &IndexView, request: &Query) -> (Vec<String>, Option<usize>) {
    let result = view.search(request).unwrap();
    (
        result
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect(),
        result.count,
    )
}

fn assert_published_dictionaries_archived(view: &IndexView) {
    view.0.validate_ownership().unwrap();
    for collection in view.repository().collections() {
        assert!(
            collection.postings.all_archived(),
            "base posting dictionary reverted to a native builder"
        );
        for custom in collection.custom_postings.values() {
            assert!(
                custom.index.is_archived(),
                "base custom posting dictionary reverted to a native builder"
            );
        }
        if let Some(inference) = &collection.inference {
            assert!(
                inference.indexes.values().all(|index| index.is_archived()),
                "base inferred posting dictionary reverted to a native builder"
            );
        }
    }
    for collection in 0..view
        .repository()
        .collections()
        .len()
    {
        for segment in view
            .segments(crate::model::CollectionId(collection as u32))
            .unwrap()
            .iter()
        {
            assert!(
                segment.index.postings.all_archived(),
                "published segment posting dictionary reverted to a native builder"
            );
            assert!(
                segment
                    .index
                    .custom_postings
                    .values()
                    .all(|custom| custom.index.is_archived()),
                "published custom posting dictionary reverted to a native builder"
            );
            if let Some(inference) = &segment.index.inference {
                assert!(
                    inference.indexes.values().all(|index| index.is_archived()),
                    "published inferred posting dictionary reverted to a native builder"
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn assert_fixture_queries(
    index: &Index,
    root: &std::path::Path,
    definitions: &[CollectionDefinition],
    expected_year: &[&str],
    expected_ge: &[&str],
    expected_old: &[&str],
    expected_new: &[&str],
    expected_common: &[&str],
) {
    let view = index.view().unwrap();
    let sorted = query(None, None, 100, 0);
    let ge = query(None, Some("year >= 15"), 100, 0);
    let old = query(Some("token-old"), None, 100, 0);
    let new = query(Some("token-new"), None, 100, 0);
    let common = query(Some("token-common"), None, 100, 0);
    for (request, expected) in [
        (&sorted, expected_year),
        (&ge, expected_ge),
        (&old, expected_old),
        (&new, expected_new),
        (&common, expected_common),
    ] {
        let (actual, count) = ids(&view, request);
        assert_eq!(
            actual,
            expected,
            "fts={:?} filter={:?}",
            request.fts.as_ref().map(|fts| fts.literal.as_str()),
            request.filter
        );
        assert_eq!(count, Some(expected.len()));
        let reference = view.search_reference(request).unwrap();
        assert_eq!(
            actual,
            reference
                .hits
                .into_iter()
                .map(|hit| hit.record_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(count, reference.count);
    }
    for offset in 0..expected_year.len() {
        let page = query(None, None, 1, offset);
        let (actual, count) = ids(&view, &page);
        assert_eq!(actual, [expected_year[offset]]);
        assert_eq!(count, Some(expected_year.len()));
    }

    let fresh = Index::build(root, definitions.to_vec(), prototype_options()).unwrap();
    let fresh_view = fresh.view().unwrap();
    for request in [&sorted, &ge, &old, &new, &common] {
        assert_eq!(
            ids(&view, request),
            ids(&fresh_view, request),
            "mutation and fresh reconstruction diverged for {:?}",
            request.filter
        );
    }
    fresh.shutdown().unwrap();
}

fn assert_scalar_oracle(
    native: &BTreeMap<ScalarKey, RoaringBitmap>,
) -> PostingDictionary<StoredScalarKey> {
    let archived = PostingDictionary::from_public(native.clone()).freeze().unwrap();
    assert!(archived.is_archived());
    assert_eq!(archived.len(), native.len());
    let mut cursor = archived.range(Unbounded, Unbounded, false);
    for (expected_key, expected_posting) in native {
        let (actual_key, actual_posting) = cursor.next().unwrap();
        assert_eq!(&actual_key, expected_key);
        assert_eq!(actual_posting, expected_posting);
    }
    assert!(cursor.next().is_none());
    archived
}

#[test]
fn rkyv_scalar_dictionary_empty_singleton_and_multi_match_native_oracle() {
    let empty = BTreeMap::new();
    assert_scalar_oracle(&empty);

    let singleton = BTreeMap::from([(
        ScalarKey::String("only".into()),
        bitmap([3]),
    )]);
    assert_scalar_oracle(&singleton);

    let multi = BTreeMap::from([
        (ScalarKey::String("a".into()), bitmap([1, 7])),
        (ScalarKey::String("é".into()), bitmap([2])),
        (ScalarKey::Id("a".into()), bitmap([4])),
        (ScalarKey::Number(Number::integer(9)), bitmap([5])),
        (ScalarKey::Bool(false), bitmap([6])),
    ]);
    assert_scalar_oracle(&multi);
}

#[test]
fn rkyv_forward_and_descending_bounds_match_native_oracle() {
    let native = (0..16)
        .map(|value| {
            (
                ScalarKey::Number(Number::integer(value)),
                bitmap([value as u32]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let archived = assert_scalar_oracle(&native);

    for (lower, upper) in [
        (Included(ScalarKey::Number(Number::integer(3))), Excluded(ScalarKey::Number(Number::integer(9)))),
        (Excluded(ScalarKey::Number(Number::integer(3))), Included(ScalarKey::Number(Number::integer(9)))),
        (Unbounded, Excluded(ScalarKey::Number(Number::integer(4)))),
        (Included(ScalarKey::Number(Number::integer(12))), Unbounded),
    ] {
        let expected = native
            .range((lower.clone(), upper.clone()))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for descending in [false, true] {
            let mut cursor = archived.range(lower.clone(), upper.clone(), descending);
            let mut actual = Vec::new();
            while let Some((key, _)) = cursor.next() {
                actual.push(key);
            }
            if descending {
                actual.reverse();
            }
            assert_eq!(actual, expected);
        }
    }

    let reversed = archived.range(
        Excluded(ScalarKey::Number(Number::integer(9))),
        Excluded(ScalarKey::Number(Number::integer(3))),
        false,
    );
    assert_eq!(reversed.into_iter_snapshot().len(), 0);
}

trait CursorSnapshot {
    fn into_iter_snapshot(self) -> Vec<ScalarKey>;
}

impl CursorSnapshot for crate::posting::PostingValueCursor<'_> {
    fn into_iter_snapshot(mut self) -> Vec<ScalarKey> {
        let mut values = Vec::new();
        while let Some((key, _)) = self.next() {
            values.push(key);
        }
        values
    }
}

#[test]
fn rkyv_numeric_and_scalar_keys_preserve_exact_order_and_types() {
    let exact_small = Number::ExactDecimal(
        ExactDecimal::parse("-1.2345678901234567890123456789e-120").unwrap(),
    );
    let exact_large =
        Number::ExactDecimal(ExactDecimal::parse("9.999999999999999999e1000").unwrap());
    let values = vec![
        Number::Integer(0),
        Number::Float(-0.0),
        Number::Float(1.5),
        Number::Integer(1),
        Number::Float(1.0),
        Number::Integer(1i128 << 53),
        Number::Float((1i128 << 53) as f64),
        Number::Integer(i128::MAX),
        Number::Integer(i128::MIN),
        exact_small,
        exact_large,
    ];
    let native = values
        .into_iter()
        .enumerate()
        .map(|(slot, value)| (value, bitmap([slot as u32])))
        .collect::<BTreeMap<Number, RoaringBitmap>>();
    let archived: PostingDictionary<PostingNumberKey> = PostingDictionary::from_public(native.clone())
        .freeze()
        .unwrap();
    let mut cursor = archived.range(Unbounded, Unbounded, false);
    for expected in native.keys() {
        assert_eq!(&cursor.next().unwrap().0, expected);
    }
    assert!(cursor.next().is_none());

    let date = Date::from_calendar_date(2024, Month::January, 2).unwrap();
    let datetime = OffsetDateTime::parse("2024-01-02T03:04:05+02:30", &Rfc3339).unwrap();
    let scalars = BTreeMap::from([
        (ScalarKey::String("same".into()), bitmap([0])),
        (ScalarKey::Id("same".into()), bitmap([1])),
        (ScalarKey::Date(date), bitmap([2])),
        (ScalarKey::Datetime(datetime), bitmap([3])),
        (ScalarKey::Unixtime(datetime.unix_timestamp()), bitmap([4])),
    ]);
    let archived = assert_scalar_oracle(&scalars);
    assert_eq!(
        archived
            .get(&ScalarKey::Datetime(datetime))
            .unwrap(),
        &bitmap([3])
    );
}

#[test]
fn rkyv_descending_4096_key_pages_use_archived_reverse_directory() {
    let native = (0..4096)
        .map(|value| {
            (
                ScalarKey::Number(Number::integer(value)),
                bitmap([value as u32]),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let archived: PostingDictionary<StoredScalarKey> = PostingDictionary::from_public(native)
        .freeze()
        .unwrap();
    for (offset, limit) in [(0, 1), (1, 1), (127, 64), (4095, 1)] {
        let mut cursor = archived.range(Unbounded, Unbounded, true);
        for _ in 0..offset {
            cursor.next().unwrap();
        }
        for expected in (0..limit).map(|index| 4095 - offset - index) {
            let (key, _) = cursor.next().unwrap();
            assert_eq!(key, ScalarKey::Number(Number::integer(expected)));
        }
    }

    for _ in 0..100 {
        assert_eq!(
            archived.get(&ScalarKey::Number(Number::integer(2048))),
            Some(&bitmap([2048]))
        );
    }
}

#[test]
fn rkyv_repository_and_lifecycle_defaults_publish_archived_components() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-defaults-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"alpha"}"#,
    )
    .unwrap();
    let definition = prototype_definition();
    let repository = Repository::open_with_options(
        &root,
        vec![definition.clone()],
        prototype_options(),
    )
    .unwrap();
    assert!(
        repository.collections()[0]
            .postings
            .all_archived(),
        "Repository dictionaries must be archived"
    );

    let index = Index::build(
        &root,
        vec![definition.clone()],
        prototype_options(),
    )
    .unwrap();
    let view = index.view().unwrap();
    assert_published_dictionaries_archived(&view);
    assert_eq!(
        view.file_handle_for_path(std::path::Path::new("data/a.json"))
            .unwrap()
            .instance_id,
        index.instance_id()
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_native_builder_is_rejected_at_publication_boundary() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-publication-boundary-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"boundary"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let view = index.view().unwrap();
    let revision = view.revision();
    let published = &view.segments(crate::model::CollectionId(0)).unwrap()[0];
    let mut native = (*published.index).clone();
    native.postings.thaw_storage().unwrap();
    let error = match crate::native_segment::PublishedSegment::checked(
        published.id,
        Arc::new(native),
        published.active.clone(),
    ) {
        Ok(_) => panic!("native builder unexpectedly remained publishable"),
        Err(error) => error,
    };
    assert!(error.contains("native posting dictionary"));
    assert_eq!(index.view().unwrap().revision(), revision);
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_concrete_add_replace_delete_compact_matches_fresh_reconstruction() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-lifecycle-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let definitions = vec![prototype_definition()];
    let index = Index::build(&root, definitions.clone(), prototype_options()).unwrap();
    assert_fixture_queries(
        &index, &root, &definitions, &[], &[], &[], &[], &[],
    );

    let write = |name: &str, value: &str| {
        let path = root.join(format!("data/{name}.json"));
        std::fs::write(&path, value).unwrap();
        path
    };

    let a = write("a", r#"{"id":"a","year":10,"title":"alpha token-old"}"#);
    index.add_file(&a).unwrap();
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["a"],
        &[],
        &["a"],
        &[],
        &[],
    );

    let b = write("b", r#"{"id":"b","year":20,"title":"beta token-common"}"#);
    index.add_file(&b).unwrap();
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["a", "b"],
        &["b"],
        &["a"],
        &[],
        &["b"],
    );

    let c = write("c", r#"{"id":"c","year":30,"title":"gamma token-common"}"#);
    let c_handle = index.add_file(&c).unwrap().file;
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["a", "b", "c"],
        &["b", "c"],
        &["a"],
        &[],
        &["b", "c"],
    );

    let mut b_handle = index.view().unwrap().file_handle_for_path(&b).unwrap();
    std::fs::write(
        &b,
        r#"{"id":"b","year":5,"title":"beta token-new"}"#,
    )
    .unwrap();
    b_handle = index.update_file(b_handle, &b).unwrap().file;
    assert_published_dictionaries_archived(&index.view().unwrap());
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["b", "a", "c"],
        &["c"],
        &["a"],
        &["b"],
        &["c"],
    );

    let a_handle = index.view().unwrap().file_handle_for_path(&a).unwrap();
    std::fs::remove_file(&a).unwrap();
    index.delete_file(a_handle).unwrap();
    assert_published_dictionaries_archived(&index.view().unwrap());
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["b", "c"],
        &["c"],
        &[],
        &["b"],
        &["c"],
    );

    let d = write("d", r#"{"id":"d","year":15,"title":"delta token-common"}"#);
    index.add_file(&d).unwrap();
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["b", "d", "c"],
        &["d", "c"],
        &[],
        &["b"],
        &["d", "c"],
    );

    index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_published_dictionaries_archived(&index.view().unwrap());
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["b", "d", "c"],
        &["d", "c"],
        &[],
        &["b"],
        &["d", "c"],
    );

    std::fs::write(
        &c,
        r#"{"id":"c","year":7,"title":"gamma token-new"}"#,
    )
    .unwrap();
    let _c_handle = index.update_file(c_handle, &c).unwrap().file;
    assert_published_dictionaries_archived(&index.view().unwrap());
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["b", "c", "d"],
        &["d"],
        &[],
        &["b", "c"],
        &["d"],
    );

    std::fs::remove_file(&b).unwrap();
    index.delete_file(b_handle).unwrap();
    assert_fixture_queries(
        &index,
        &root,
        &definitions,
        &["c", "d"],
        &["d"],
        &[],
        &["c"],
        &["d"],
    );
    let c_handle = index.view().unwrap().file_handle_for_path(&c).unwrap();
    std::fs::remove_file(&c).unwrap();
    index.delete_file(c_handle).unwrap();
    let d_handle = index.view().unwrap().file_handle_for_path(&d).unwrap();
    std::fs::remove_file(&d).unwrap();
    index.delete_file(d_handle).unwrap();
    index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_published_dictionaries_archived(&index.view().unwrap());
    assert_fixture_queries(
        &index, &root, &definitions, &[], &[], &[], &[], &[],
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_multi_record_replacement_spans_segments_and_validates_ranges() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-jsonl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut definition =
        CollectionDefinition::folder("items", "data", SourceFormat::JsonLines);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    for (name, kind) in [
        ("id", LogicalType::Id),
        ("year", LogicalType::Number),
        ("title", LogicalType::String),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: name == "id",
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let options = RepositoryOptions {
        segment_policy: crate::native_segment::SegmentPolicy {
            target_records: std::num::NonZeroUsize::new(4).unwrap(),
            max_files: std::num::NonZeroUsize::new(4).unwrap(),
        },
        ..Default::default()
    };
    let index = Index::build(&root, vec![definition], options).unwrap();
    let path = root.join("data/multi.jsonl");
    let old = (0..11)
        .map(|number| {
            format!(
                "{{\"id\":\"r{number:02}\",\"year\":{number},\"title\":\"old-only\"}}\n"
            )
        })
        .collect::<String>();
    std::fs::write(&path, &old).unwrap();
    let sentinel = root.join("data/sentinel.jsonl");
    std::fs::write(
        &sentinel,
        b"{\"id\":\"sentinel\",\"year\":999,\"title\":\"sentinel-only\"}\n",
    )
    .unwrap();
    index.add_file(&path).unwrap();
    index.add_file(&sentinel).unwrap();
    let view = index.view().unwrap();
    let initial_multi_segments = view
        .segments(crate::model::CollectionId(0))
        .unwrap()
        .iter()
        .filter(|segment| {
            segment
                .index
                .files
                .iter()
                .any(|file| file.entry.relative_path.as_path() == std::path::Path::new("data/multi.jsonl"))
        })
        .count();
    assert!(
        initial_multi_segments >= 3,
        "multi segments={initial_multi_segments}, all segments={:?}",
        view.segments(crate::model::CollectionId(0))
            .unwrap()
            .iter()
            .map(|segment| (
                segment.id.get(),
                segment.index.record_count(),
                segment
                    .index
                    .files
                    .iter()
                    .map(|file| file.entry.relative_path.as_path().to_path_buf())
                    .collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>()
    );
    let handle = view
        .file_handle_for_path(std::path::Path::new("data/multi.jsonl"))
        .unwrap();
    drop(view);

    let replacement = (0..7)
        .map(|number| {
            format!(
                "{{\"id\":\"r{number:02}\",\"year\":{},\"title\":\"new-only with changed line length {number}\"}}\n",
                100 + number
            )
        })
        .collect::<String>();
    std::fs::write(&path, replacement).unwrap();
    let handle = index.update_file(handle, &path).unwrap().file;
    let view = index.view().unwrap();
    assert_eq!(view.file_record_count(handle.file_id), Some(7));
    let old_query = query(Some("old-only"), None, 100, 0);
    assert_eq!(ids(&view, &old_query).0, Vec::<String>::new());
    let new_query = query(Some("new-only"), None, 100, 0);
    assert_eq!(
        ids(&view, &new_query).0,
        [
            "r00", "r01", "r02", "r03", "r04", "r05", "r06"
        ]
    );
    let removed = query(None, Some("id = \"r07\""), 100, 0);
    assert_eq!(ids(&view, &removed).0, Vec::<String>::new());
    let source = std::fs::read(&path).unwrap();
    let result = view.search(&new_query).unwrap();
    for hit in result.hits {
        let start = usize::try_from(hit.byte_start).unwrap();
        let end = usize::try_from(hit.byte_end).unwrap();
        let bytes = source.get(start..end).unwrap();
        let parsed = serde_json::from_slice::<serde_json::Value>(bytes).unwrap();
        assert_eq!(parsed["id"], hit.record_id);
    }
    drop(view);

    std::fs::remove_file(&path).unwrap();
    index.delete_file(handle).unwrap();
    let view = index.view().unwrap();
    assert_eq!(
        ids(&view, &query(None, None, 100, 0)).0,
        ["sentinel"]
    );
    drop(view);
    index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_eq!(
        ids(
            &index.view().unwrap(),
            &query(None, None, 100, 0)
        )
        .0,
        ["sentinel"]
    );
    let pinned = index.view().unwrap();
    let revision = pinned.revision();
    let owner = Arc::clone(
        &pinned
            .segments(crate::model::CollectionId(0))
            .unwrap()[0],
    );
    drop(pinned);
    let no_op = index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_eq!(no_op.revision, revision);
    assert_eq!(no_op.segments_merged, 0);
    let pinned = index.view().unwrap();
    assert!(Arc::ptr_eq(
        &owner,
        &pinned.segments(crate::model::CollectionId(0)).unwrap()[0]
    ));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_repeated_single_file_updates_keep_one_live_segment() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-repeat-update-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/only.json");
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    std::fs::write(
        &path,
        br#"{"id":"only","year":0,"title":"revision 0"}"#,
    )
    .unwrap();
    let mut handle = index.add_file(&path).unwrap().file;
    for revision in 1..=1_000 {
        std::fs::write(
            &path,
            format!(
                "{{\"id\":\"only\",\"year\":{revision},\"title\":\"revision {revision}\"}}"
            ),
        )
        .unwrap();
        handle = index.update_file(handle, &path).unwrap().file;
        let view = index.view().unwrap();
        assert_eq!(
            view.segments(crate::model::CollectionId(0))
                .unwrap()
                .iter()
                .filter(|segment| {
                    segment
                        .index
                        .file(handle.file_id)
                        .is_some_and(|_| segment.active.len() > 0)
                })
                .count(),
            1,
            "update {revision} retained obsolete live-segment descriptors"
        );
        assert_eq!(view.search(&query(None, None, 10, 0)).unwrap().count, Some(1));
        view.0.validate_ownership().unwrap();
    }
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_delete_all_records_removes_payload_without_reusing_file_id() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-delete-all-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let first = root.join("data/first.json");
    let replacement = root.join("data/replacement.json");
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    std::fs::write(
        &first,
        br#"{"id":"first","year":1,"title":"delete all"}"#,
    )
    .unwrap();
    let old = index.add_file(&first).unwrap().file;
    std::fs::remove_file(&first).unwrap();
    index.delete_file(old).unwrap();
    let view = index.view().unwrap();
    assert_eq!(view.search(&query(None, None, 10, 0)).unwrap().count, Some(0));
    assert!(
        view.segments(crate::model::CollectionId(0))
            .unwrap()
            .iter()
            .all(|segment| segment.index.file(old.file_id).is_none())
    );
    view.0.validate_ownership().unwrap();
    drop(view);
    std::fs::write(
        &replacement,
        br#"{"id":"second","year":2,"title":"new identity"}"#,
    )
    .unwrap();
    let next = index.add_file(&replacement).unwrap().file;
    assert_ne!(next.file_id, old.file_id);
    assert!(next.file_id.0 > old.file_id.0);
    assert_eq!(
        index
            .view()
            .unwrap()
            .search(&query(Some("new identity"), None, 10, 0))
            .unwrap()
            .count,
        Some(1)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_same_path_update_rebuilds_one_metadata_and_ownership_shard() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-path-locality-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/same.json");
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    std::fs::write(
        &path,
        br#"{"id":"same","year":1,"title":"first content"}"#,
    )
    .unwrap();
    let handle = index.add_file(&path).unwrap().file;
    crate::storage_usage::StorageWorkCounters::reset();
    std::fs::write(
        &path,
        br#"{"id":"same","year":2,"title":"second content"}"#,
    )
    .unwrap();
    index.update_file(handle, &path).unwrap();
    let counters = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(counters.path_shards_rebuilt, 0);
    assert_eq!(counters.metadata_shards_rebuilt, 1);
    assert_eq!(counters.ownership_shards_rebuilt, 1);
    assert!(counters.metadata_rows_examined <= 1_024);
    assert_eq!(
        index
            .view()
            .unwrap()
            .search(&query(Some("second content"), None, 10, 0))
            .unwrap()
            .count,
        Some(1)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_rename_rebuilds_only_each_affected_path_bucket_once() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-rename-locality-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let original = root.join("data/original.json");
    std::fs::write(
        &original,
        br#"{"id":"rename","year":1,"title":"rename locality"}"#,
    )
    .unwrap();
    let handle = index.add_file(&original).unwrap().file;
    let source_bucket = crate::lifecycle_catalog::shard(
        &crate::path_repr::query_path(std::path::Path::new("data/original.json")),
    );
    let mut same_bucket = None;
    let mut other_bucket = None;
    for number in 0..10_000 {
        let relative = format!("data/move-{number}.json");
        let bucket = crate::lifecycle_catalog::shard(
            &crate::path_repr::query_path(std::path::Path::new(&relative)),
        );
        if bucket == source_bucket && same_bucket.is_none() {
            same_bucket = Some(root.join(relative));
        } else if bucket != source_bucket && other_bucket.is_none() {
            other_bucket = Some(root.join(relative));
        }
        if same_bucket.is_some() && other_bucket.is_some() {
            break;
        }
    }
    let same_bucket = same_bucket.unwrap();
    std::fs::rename(&original, &same_bucket).unwrap();
    crate::storage_usage::StorageWorkCounters::reset();
    let handle = index.move_file(handle, &same_bucket).unwrap().file;
    let counters = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(counters.path_shards_rebuilt, 1);
    assert!(counters.path_entries_examined <= 1_024);
    assert_eq!(counters.metadata_shards_rebuilt, 1);

    let other_bucket = other_bucket.unwrap();
    std::fs::rename(&same_bucket, &other_bucket).unwrap();
    crate::storage_usage::StorageWorkCounters::reset();
    index.move_file(handle, &other_bucket).unwrap();
    let counters = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(counters.path_shards_rebuilt, 2);
    assert!(counters.path_entries_examined <= 2_048);
    assert_eq!(counters.metadata_shards_rebuilt, 1);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_large_catalog_update_examines_only_the_affected_metadata_shard() {
    let mut files = crate::inventory::FileInventory::new(std::path::PathBuf::from("/synthetic"));
    let revision = crate::inventory::SourceRevision {
        modified_ns: 1,
        size: 2,
    };
    for number in 0..100_000 {
        files
            .add(
                std::path::PathBuf::from(format!("data/file-{number:06}.json")),
                revision,
                None,
                crate::model::CollectionId(0),
            )
            .unwrap();
    }
    let catalog = crate::lifecycle_catalog::FileCatalog::from_parts(
        Arc::new(files),
        vec![None; 100_000],
        [0; 32],
    );
    let file = crate::model::FileId(54_321);
    let mut entry = catalog.entry_owned(file).unwrap();
    entry.revision.modified_ns = 3;
    crate::storage_usage::StorageWorkCounters::reset();
    let _updated = catalog.replaced(entry, None, None).unwrap();
    let counters = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(counters.metadata_shards_rebuilt, 1);
    assert_eq!(counters.path_shards_rebuilt, 0);
    assert!(
        counters.metadata_rows_examined <= 1_024,
        "examined {} rows for one 1,024-ID shard",
        counters.metadata_rows_examined
    );
}

#[test]
fn rkyv_one_file_routes_invalidation_across_three_segments_and_two_collections() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-routed-delete-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut first = prototype_definition();
    first.format = SourceFormat::JsonLines;
    first.name = "first".into();
    let mut second = prototype_definition();
    second.format = SourceFormat::JsonLines;
    second.name = "second".into();
    let options = RepositoryOptions {
        segment_policy: crate::native_segment::SegmentPolicy {
            target_records: std::num::NonZeroUsize::new(2).unwrap(),
            max_files: std::num::NonZeroUsize::new(2).unwrap(),
        },
        ..Default::default()
    };
    let index = Index::build(&root, vec![first, second], options).unwrap();
    let path = root.join("data/multi.jsonl");
    let body = (0..9)
        .map(|number| {
            format!(
                "{{\"id\":\"r{number}\",\"year\":{number},\"title\":\"routed file\"}}\n"
            )
        })
        .collect::<String>();
    std::fs::write(&path, body).unwrap();
    let handle = index.add_file(&path).unwrap().file;
    let view = index.view().unwrap();
    let mut routed_segments = std::collections::BTreeSet::new();
    for collection in 0..2 {
        for segment in view
            .segments(crate::model::CollectionId(collection))
            .unwrap()
            .iter()
        {
            if segment.index.file(handle.file_id).is_some() {
                routed_segments.insert((collection, segment.id));
            }
        }
    }
    assert!(routed_segments.len() >= 6);
    assert_eq!(view.file_record_count(handle.file_id), Some(18));
    view.0.validate_ownership().unwrap();
    drop(view);
    std::fs::remove_file(&path).unwrap();
    index.delete_file(handle).unwrap();
    let view = index.view().unwrap();
    for collection in 0..2 {
        assert_eq!(
            view.search(&sequence_query(
                if collection == 0 { "first" } else { "second" },
                None,
                0,
                10
            ))
            .unwrap()
            .count,
            Some(0)
        );
        assert!(view.segments(crate::model::CollectionId(collection)).unwrap().iter().all(
            |segment| segment.index.file(handle.file_id).is_none()
        ));
    }
    view.0.validate_ownership().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_relocated_survivor_uses_rewritten_ownership_route() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-relocated-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let survivor = root.join("data/survivor.jsonl");
    let main = root.join("data/main.jsonl");
    let mut definition = prototype_definition();
    definition.format = SourceFormat::JsonLines;
    let index = Index::build(
        &root,
        vec![definition],
        RepositoryOptions {
            segment_policy: crate::native_segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(2).unwrap(),
                max_files: std::num::NonZeroUsize::new(2).unwrap(),
            },
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::write(
        &survivor,
        b"{\"id\":\"survivor\",\"year\":1,\"title\":\"survivor old\"}\n",
    )
    .unwrap();
    std::fs::write(
        &main,
        b"{\"id\":\"main-a\",\"year\":2,\"title\":\"main old\"}\n{\"id\":\"main-b\",\"year\":3,\"title\":\"main old\"}\n",
    )
    .unwrap();
    let survivor_handle = index.add_file(&survivor).unwrap().file;
    let main_handle = index.add_file(&main).unwrap().file;
    let old_route = index
        .view()
        .unwrap()
        .segments(crate::model::CollectionId(0))
        .unwrap()
        .iter()
        .find(|segment| segment.index.file(survivor_handle.file_id).is_some())
        .unwrap()
        .id;
    std::fs::write(
        &main,
        b"{\"id\":\"main-a\",\"year\":22,\"title\":\"main changed\"}\n",
    )
    .unwrap();
    index.update_file(main_handle, &main).unwrap();
    let view = index.view().unwrap();
    let new_route = view
        .segments(crate::model::CollectionId(0))
        .unwrap()
        .iter()
        .find(|segment| segment.index.file(survivor_handle.file_id).is_some())
        .unwrap()
        .id;
    assert_ne!(new_route, old_route);
    view.0.validate_ownership().unwrap();
    drop(view);
    std::fs::write(
        &survivor,
        b"{\"id\":\"survivor\",\"year\":11,\"title\":\"survivor updated\"}\n",
    )
    .unwrap();
    let survivor_handle = index
        .update_file(survivor_handle, &survivor)
        .unwrap()
        .file;
    assert_eq!(
        index
            .view()
            .unwrap()
            .search(&query(Some("survivor updated"), None, 10, 0))
            .unwrap()
            .count,
        Some(1)
    );
    index.delete_file(survivor_handle).unwrap();
    let view = index.view().unwrap();
    assert_eq!(
        view.search(&query(Some("survivor updated"), None, 10, 0))
            .unwrap()
            .count,
        Some(0)
    );
    view.0.validate_ownership().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_mutation_strategy_and_segment_ids_are_transaction_specific() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-strategy-flags-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let first = root.join("data/first.json");
    let second = root.join("data/second.json");
    let third = root.join("data/third.json");
    std::fs::write(&first, br#"{"id":"first","year":1,"title":"one"}"#).unwrap();
    let first_result = index.add_file(&first).unwrap();
    assert_eq!(
        first_result.strategy,
        crate::lifecycle_mutation::FileMutationStrategy::AppendReplacement
    );
    std::fs::write(&second, br#"{"id":"second","year":2,"title":"two"}"#).unwrap();
    let second_result = index.add_file(&second).unwrap();
    assert_eq!(
        second_result.strategy,
        crate::lifecycle_mutation::FileMutationStrategy::RebuildSmallestSegment
    );
    std::fs::write(&third, br#"{"id":"third","year":3,"title":"three"}"#).unwrap();
    let third_result = index.add_file(&third).unwrap();
    assert_eq!(
        third_result.strategy,
        crate::lifecycle_mutation::FileMutationStrategy::AppendReplacement
    );
    let view = index.view().unwrap();
    for group in &view.0.segments {
        for segment in group.iter() {
            assert_eq!(segment.id, segment.index.id);
        }
    }
    view.0.validate_ownership().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_zero_record_and_malformed_files_keep_current_metadata_and_routes() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-empty-broken-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut definition = prototype_definition();
    definition.format = SourceFormat::JsonLines;
    let index = Index::build(&root, vec![definition], prototype_options()).unwrap();
    let empty = root.join("data/empty.jsonl");
    let broken = root.join("data/broken.jsonl");
    std::fs::write(&empty, b"").unwrap();
    std::fs::write(&broken, b"{not-json\n").unwrap();
    let empty_handle = index.add_file(&empty).unwrap().file;
    let broken_handle = index.add_file(&broken).unwrap().file;
    let view = index.view().unwrap();
    assert_eq!(view.file_record_count(empty_handle.file_id), Some(0));
    assert_eq!(view.file_record_count(broken_handle.file_id), Some(1));
    let empty_entry = view.file_entry_owned(empty_handle.file_id).unwrap();
    let broken_entry = view.file_entry_owned(broken_handle.file_id).unwrap();
    assert_eq!(empty_entry.relative_path.as_path(), std::path::Path::new("data/empty.jsonl"));
    assert_eq!(broken_entry.relative_path.as_path(), std::path::Path::new("data/broken.jsonl"));
    assert!(!broken_entry.diagnostics.is_empty());
    assert_ne!(broken_entry.status, crate::inventory::FileStatus::Valid);
    view.0.validate_ownership().unwrap();
    assert_eq!(view.search(&query(None, None, 10, 0)).unwrap().count, Some(1));
    drop(view);
    index.delete_file(empty_handle).unwrap();
    assert!(
        index
            .view()
            .unwrap()
            .segments(crate::model::CollectionId(0))
            .unwrap()
            .iter()
            .all(|segment| segment.index.file(empty_handle.file_id).is_none())
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_deterministic_mutation_sequences_match_fresh_rebuilds_and_models() {
    for seed in [1u64, 7, 19, 41, 97] {
        let root = std::env::temp_dir().join(format!(
            "objekte-rkyv-seed-{seed}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let mut first = prototype_definition();
        first.name = "first".into();
        let mut second = prototype_definition();
        second.name = "second".into();
        let definitions = vec![first, second];
        let options = RepositoryOptions {
            verified_cache_bytes: if seed % 2 == 0 { 2 * 1024 * 1024 } else { 0 },
            ..prototype_options()
        };
        let index = Index::build(&root, definitions.clone(), options).unwrap();
        let mut model = BTreeMap::<std::path::PathBuf, (String, i64)>::new();
        let mut files = Vec::<(std::path::PathBuf, crate::lifecycle::FileHandle)>::new();
        let mut random = seed;
        let mut serial = 0usize;

        for operation in 0..100 {
            random = random
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let action = random % 100;
            if files.is_empty() || (action < 45 && files.len() < 8) {
                let id = format!("s{seed}_{serial}");
                serial += 1;
                let year = (random % 40) as i64;
                let path = root.join(format!("data/{id}.json"));
                std::fs::write(
                    &path,
                    format!("{{\"id\":\"{id}\",\"year\":{year},\"title\":\"seed {seed} add\"}}"),
                )
                .unwrap();
                let handle = index.add_file(&path).unwrap().file;
                model.insert(path.clone(), (id, year));
                files.push((path, handle));
            } else if action < 75 {
                let position = (random as usize) % files.len();
                let id = format!("s{seed}_{serial}");
                serial += 1;
                let year = (random % 40) as i64;
                std::fs::write(
                    &files[position].0,
                    format!("{{\"id\":\"{id}\",\"year\":{year},\"title\":\"seed {seed} replace\"}}"),
                )
                .unwrap();
                let handle = index
                    .update_file(files[position].1, &files[position].0)
                    .unwrap()
                    .file;
                model.insert(files[position].0.clone(), (id, year));
                files[position].1 = handle;
            } else if action < 94 {
                let position = (random as usize) % files.len();
                let (path, handle) = files.remove(position);
                std::fs::remove_file(&path).unwrap();
                index.delete_file(handle).unwrap();
                model.remove(&path);
            } else {
                index
                    .compact_with_options(CompactionOptions {
                        mode: CompactionMode::Full,
                        merge_small_segments: true,
                    })
                    .unwrap();
            }

            assert_sequence_oracle(&index, &root, &definitions, &options, &model, seed, operation);
        }
        index
            .compact_with_options(CompactionOptions {
                mode: CompactionMode::Full,
                merge_small_segments: true,
            })
            .unwrap();
        assert_sequence_oracle(&index, &root, &definitions, &options, &model, seed, 100);
        index.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

fn assert_sequence_oracle(
    index: &Index,
    root: &std::path::Path,
    definitions: &[CollectionDefinition],
    options: &RepositoryOptions,
    model: &BTreeMap<std::path::PathBuf, (String, i64)>,
    seed: u64,
    operation: usize,
) {
    let view = index.view().unwrap();
    let fresh = Index::build(root, definitions.to_vec(), *options).unwrap();
    let fresh_view = fresh.view().unwrap();
    let mut expected = model.values().collect::<Vec<_>>();
    expected.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    let expected_ids = expected
        .iter()
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    let expected_ge = expected
        .iter()
        .filter(|(_, year)| *year >= 15)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for collection in ["first", "second"] {
        let sorted = sequence_query(collection, None, 0, 100);
        let ge = sequence_query(collection, Some("year >= 15"), 0, 100);
        let (actual, count) = ids(&view, &sorted);
        let (fresh_actual, fresh_count) = ids(&fresh_view, &sorted);
        assert_eq!(
            actual, expected_ids,
            "seed={seed} operation={operation} collection={collection}"
        );
        assert_eq!(actual, fresh_actual);
        assert_eq!(count, Some(expected_ids.len()));
        assert_eq!(count, fresh_count);
        let (actual, count) = ids(&view, &ge);
        assert_eq!(
            actual, expected_ge,
            "seed={seed} operation={operation} collection={collection} predicate"
        );
        assert_eq!(count, Some(expected_ge.len()));
        assert_eq!((actual, count), ids(&fresh_view, &ge));
        for offset in 0..expected_ids.len().min(4) {
            let page = sequence_query(collection, None, offset, 1);
            assert_eq!(ids(&view, &page), (vec![expected_ids[offset].clone()], Some(expected_ids.len())));
            assert_eq!(ids(&view, &page), ids(&fresh_view, &page));
        }
        for segment in view
            .segments(if collection == "first" {
                crate::model::CollectionId(0)
            } else {
                crate::model::CollectionId(1)
            })
            .unwrap()
            .iter()
        {
            for slot in 0..segment.index.record_count() {
                if segment.active.contains(crate::model::RecordSlot(slot as u32)) {
                    assert!(
                        segment
                            .index
                            .record(crate::model::RecordSlot(slot as u32))
                            .is_some()
                    );
                }
            }
        }
    }
    for path in model.keys() {
        assert!(view.file_handle_for_path(path).is_some());
    }
    drop(fresh_view);
    fresh.shutdown().unwrap();
}

fn sequence_query(
    collection: &str,
    filter: Option<&str>,
    offset: usize,
    limit: usize,
) -> Query {
    Query {
        collections: vec![collection.into()],
        fts: None,
        filter: filter.map(|filter| parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("year", false)],
            limit,
            offset,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

#[test]
fn rkyv_build_query_and_compaction_do_not_write_files() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-write-audit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"audit alpha"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("data/b.json"),
        br#"{"id":"b","year":20,"title":"audit beta"}"#,
    )
    .unwrap();
    let snapshot = || {
        let mut files = std::fs::read_dir(root.join("data"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (path.clone(), std::fs::read(path).unwrap())
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        files
    };
    let before = snapshot();
    let index = Index::build(
        &root,
        vec![prototype_definition()],
        prototype_options(),
    )
    .unwrap();
    let request = query(Some("audit"), None, 10, 0);
    assert_eq!(index.view().unwrap().search(&request).unwrap().count, Some(2));
    index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_eq!(snapshot(), before);
    assert_eq!(
        std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        [std::ffi::OsString::from("data")]
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_source_free_compaction_removes_tombstones_without_source_opens() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-source-free-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let paths = ["a", "b", "c"].map(|name| root.join(format!("data/{name}.json")));
    for (number, path) in paths.iter().enumerate() {
        std::fs::write(
            path,
            format!(
                "{{\"id\":\"{number}\",\"year\":{number},\"title\":\"source-free\"}}"
            ),
        )
        .unwrap();
    }
    let options = RepositoryOptions {
        segment_policy: crate::native_segment::SegmentPolicy {
            target_records: std::num::NonZeroUsize::new(1).unwrap(),
            max_files: std::num::NonZeroUsize::new(1).unwrap(),
        },
        ..Default::default()
    };
    let index = Index::build(&root, vec![prototype_definition()], options).unwrap();
    let handles = paths
        .iter()
        .map(|path| index.view().unwrap().file_handle_for_path(path).unwrap())
        .collect::<Vec<_>>();
    std::fs::remove_file(&paths[1]).unwrap();
    index.delete_file(handles[1]).unwrap();
    let full = index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_eq!(full.remaining_retired_slots, 0);
    let view = index.view().unwrap();
    let result = view
        .search(&query(None, Some("year >= 0"), 10, 0))
        .unwrap();
    assert_eq!(result.count, Some(2));
    assert_eq!(result.counters.files_opened, 0);
    assert_eq!(result.counters.record_parses, 0);
    drop(view);
    let revision = index.view().unwrap().revision();
    let no_op = index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_eq!(no_op.revision, revision);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
