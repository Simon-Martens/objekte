use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::Arc;

use roaring::RoaringBitmap;
use time::{Date, Month, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::decimal::ExactDecimal;
use crate::lifecycle::{CompactionMode, CompactionOptions, Index, IndexView};
use crate::model::LogicalType;
use crate::model::Number;
use crate::posting::{PostingDictionary, PostingNumberKey, ScalarKey, StoredScalarKey};
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::FieldDefinition;
use crate::repository::{CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions};
use crate::source::SourceFormat;
use crate::workload::{ReturnMode, SortField};

fn bitmap(slots: impl IntoIterator<Item = u32>) -> RoaringBitmap {
    slots.into_iter().collect()
}

fn prototype_definition() -> CollectionDefinition {
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
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
        result.hits.into_iter().map(|hit| hit.record_id).collect(),
        result.count,
    )
}

fn assert_published_dictionaries(view: &IndexView) {
    view.0.validate_ownership().unwrap();
    for collection in view.repository().collections() {
        assert!(
            collection.postings.all_published(),
            "base posting dictionary reverted to a mutable builder"
        );
        for custom in collection.custom_postings.values() {
            assert!(
                custom.index.is_published(),
                "base custom posting dictionary reverted to a mutable builder"
            );
        }
        if let Some(inference) = &collection.inference {
            assert!(
                inference.indexes.values().all(|index| index.is_published()),
                "base inferred posting dictionary reverted to a mutable builder"
            );
        }
    }
    for collection in 0..view.repository().collections().len() {
        for segment in view
            .segments(crate::model::CollectionId(collection as u32))
            .unwrap()
            .iter()
        {
            assert!(
                segment.index.postings.all_published(),
                "published segment posting dictionary reverted to a mutable builder"
            );
            assert!(
                segment
                    .index
                    .custom_postings
                    .values()
                    .all(|custom| custom.index.is_published()),
                "published custom posting dictionary reverted to a mutable builder"
            );
            if let Some(inference) = &segment.index.inference {
                assert!(
                    inference.indexes.values().all(|index| index.is_published()),
                    "published inferred posting dictionary reverted to a mutable builder"
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
    let published = PostingDictionary::from_public(native.clone())
        .publish_native()
        .unwrap();
    assert!(published.is_native());
    assert_eq!(published.len(), native.len());
    let mut cursor = published.range(Unbounded, Unbounded, false);
    for (expected_key, expected_posting) in native {
        let (actual_key, actual_posting) = cursor.next().unwrap();
        assert_eq!(&actual_key, expected_key);
        assert_eq!(actual_posting, expected_posting);
    }
    assert!(cursor.next().is_none());
    published
}

#[test]
fn rkyv_scalar_dictionary_empty_singleton_and_multi_match_native_oracle() {
    let empty = BTreeMap::new();
    assert_scalar_oracle(&empty);

    let singleton = BTreeMap::from([(ScalarKey::String("only".into()), bitmap([3]))]);
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
        (
            Included(ScalarKey::Number(Number::integer(3))),
            Excluded(ScalarKey::Number(Number::integer(9))),
        ),
        (
            Excluded(ScalarKey::Number(Number::integer(3))),
            Included(ScalarKey::Number(Number::integer(9))),
        ),
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
    let exact_small =
        Number::ExactDecimal(ExactDecimal::parse("-1.2345678901234567890123456789e-120").unwrap());
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
    let archived: PostingDictionary<PostingNumberKey> =
        PostingDictionary::from_public(native.clone())
            .publish_native()
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
        archived.get(&ScalarKey::Datetime(datetime)).unwrap(),
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
        .publish_native()
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
    let mut repository =
        Repository::open_with_options(&root, vec![definition.clone()], prototype_options())
            .unwrap();
    assert!(
        repository.collections()[0].postings.all_native(),
        "Repository dictionaries must begin native"
    );
    let before_query = repository.search(&query(None, None, 10, 0)).unwrap().count;
    let report = repository.archive().unwrap();
    assert_eq!(
        report.representation_after,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    assert!(repository.collections()[0].postings.all_archived());
    assert_eq!(
        repository.search(&query(None, None, 10, 0)).unwrap().count,
        before_query
    );

    let index = Index::build(&root, vec![definition.clone()], prototype_options()).unwrap();
    let view = index.view().unwrap();
    assert_published_dictionaries(&view);
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Native
    );
    drop(view);
    let report = index.archive().unwrap();
    assert_eq!(
        report.representation_after,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    let view = index.view().unwrap();
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
    index.archive().unwrap();
    let view = index.view().unwrap();
    let revision = view.revision();
    let published = &view.segments(crate::model::CollectionId(0)).unwrap()[0];
    let mut mutable = (*published.index).clone();
    mutable.postings.thaw_storage().unwrap();
    let error = match crate::native_segment::PublishedSegment::checked(
        published.id,
        Arc::new(mutable),
        published.active.clone(),
    ) {
        Ok(_) => panic!("native builder unexpectedly remained publishable"),
        Err(error) => error,
    };
    assert!(error.contains("mutable query dictionary"));
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
    assert_fixture_queries(&index, &root, &definitions, &[], &[], &[], &[], &[]);

    let write = |name: &str, value: &str| {
        let path = root.join(format!("data/{name}.json"));
        std::fs::write(&path, value).unwrap();
        path
    };

    let a = write("a", r#"{"id":"a","year":10,"title":"alpha token-old"}"#);
    index.add_file(&a).unwrap();
    assert_fixture_queries(&index, &root, &definitions, &["a"], &[], &["a"], &[], &[]);

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
    std::fs::write(&b, r#"{"id":"b","year":5,"title":"beta token-new"}"#).unwrap();
    b_handle = index.update_file(b_handle, &b).unwrap().file;
    assert_published_dictionaries(&index.view().unwrap());
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
    assert_published_dictionaries(&index.view().unwrap());
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
    assert_published_dictionaries(&index.view().unwrap());
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

    std::fs::write(&c, r#"{"id":"c","year":7,"title":"gamma token-new"}"#).unwrap();
    let _c_handle = index.update_file(c_handle, &c).unwrap().file;
    assert_published_dictionaries(&index.view().unwrap());
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
    assert_published_dictionaries(&index.view().unwrap());
    assert_fixture_queries(&index, &root, &definitions, &[], &[], &[], &[], &[]);
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
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::JsonLines);
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
            format!("{{\"id\":\"r{number:02}\",\"year\":{number},\"title\":\"old-only\"}}\n")
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
            segment.index.files.iter().any(|file| {
                file.entry.relative_path.as_path() == std::path::Path::new("data/multi.jsonl")
            })
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
        ["r00", "r01", "r02", "r03", "r04", "r05", "r06"]
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
    assert_eq!(ids(&view, &query(None, None, 100, 0)).0, ["sentinel"]);
    drop(view);
    index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_eq!(
        ids(&index.view().unwrap(), &query(None, None, 100, 0)).0,
        ["sentinel"]
    );
    let pinned = index.view().unwrap();
    let revision = pinned.revision();
    let owner = Arc::clone(&pinned.segments(crate::model::CollectionId(0)).unwrap()[0]);
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
    std::fs::write(&path, br#"{"id":"only","year":0,"title":"revision 0"}"#).unwrap();
    let mut handle = index.add_file(&path).unwrap().file;
    for revision in 1..=1_000 {
        std::fs::write(
            &path,
            format!("{{\"id\":\"only\",\"year\":{revision},\"title\":\"revision {revision}\"}}"),
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
        assert_eq!(
            view.search(&query(None, None, 10, 0)).unwrap().count,
            Some(1)
        );
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
    std::fs::write(&first, br#"{"id":"first","year":1,"title":"delete all"}"#).unwrap();
    let old = index.add_file(&first).unwrap().file;
    std::fs::remove_file(&first).unwrap();
    index.delete_file(old).unwrap();
    let view = index.view().unwrap();
    assert_eq!(
        view.search(&query(None, None, 10, 0)).unwrap().count,
        Some(0)
    );
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
    std::fs::write(&path, br#"{"id":"same","year":1,"title":"first content"}"#).unwrap();
    let handle = index.add_file(&path).unwrap().file;
    crate::storage_usage::StorageWorkCounters::reset();
    std::fs::write(&path, br#"{"id":"same","year":2,"title":"second content"}"#).unwrap();
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
    let source_bucket = crate::lifecycle_catalog::shard(&crate::path_repr::query_path(
        std::path::Path::new("data/original.json"),
    ));
    let mut same_bucket = None;
    let mut other_bucket = None;
    for number in 0..10_000 {
        let relative = format!("data/move-{number}.json");
        let bucket = crate::lifecycle_catalog::shard(&crate::path_repr::query_path(
            std::path::Path::new(&relative),
        ));
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
        Arc::new(vec![None; 100_000]),
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
            format!("{{\"id\":\"r{number}\",\"year\":{number},\"title\":\"routed file\"}}\n")
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
        assert!(
            view.segments(crate::model::CollectionId(collection))
                .unwrap()
                .iter()
                .all(|segment| segment.index.file(handle.file_id).is_none())
        );
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
    let survivor_handle = index.update_file(survivor_handle, &survivor).unwrap().file;
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
    assert_eq!(
        empty_entry.relative_path.as_path(),
        std::path::Path::new("data/empty.jsonl")
    );
    assert_eq!(
        broken_entry.relative_path.as_path(),
        std::path::Path::new("data/broken.jsonl")
    );
    assert!(!broken_entry.diagnostics.is_empty());
    assert_ne!(broken_entry.status, crate::inventory::FileStatus::Valid);
    view.0.validate_ownership().unwrap();
    assert_eq!(
        view.search(&query(None, None, 10, 0)).unwrap().count,
        Some(1)
    );
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
fn rkyv_cache_free_build_keeps_native_row_caches_uninitialized() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-cache-free-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"cache free"}"#,
    )
    .unwrap();
    crate::storage_usage::StorageWorkCounters::reset();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let counters = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(
        counters.metadata_cache_initializations,
        0,
        "catalog={}, files={}, records={}",
        counters.catalog_row_cache_initializations,
        counters.file_row_cache_initializations,
        counters.record_row_cache_initializations
    );
    let view = index.view().unwrap();
    assert!(view.0.construction_registry_released());
    for segment in view.segments(crate::model::CollectionId(0)).unwrap().iter() {
        assert!(!segment.index.files.cache_initialized());
        assert!(!segment.index.records.cache_initialized());
    }
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_inventory_releases_native_path_maps_after_adoption() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-inventory-paths-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"inventory paths"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    index.archive().unwrap();
    let view = index.view().unwrap();
    assert!(view.repository().files().native_lookup_maps_released());
    assert!(view.repository().files().path_directory_archived());
    assert_eq!(
        view.repository()
            .files()
            .id_for(std::path::Path::new("data/a.json")),
        view.file_handle_for_path(std::path::Path::new("data/a.json"))
            .map(|handle| handle.file_id)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segment_rows_archive_from_borrowed_rows_round_trips() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-borrowed-rows-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"borrowed rows"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let files = index
        .view()
        .unwrap()
        .segments(crate::model::CollectionId(0))
        .unwrap()[0]
        .index
        .files
        .clone();
    let (archived_files, archived_records) = files.archive_rows().unwrap();
    assert!(archived_files.is_archived());
    assert_eq!(archived_files.len(), 1);
    assert_eq!(
        archived_records.record_id(crate::model::RecordSlot(0)),
        Some("a")
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cold_archived_metadata_queries_do_not_materialize_compatibility_rows() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-borrowed-metadata-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"borrowed metadata"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    index.archive().unwrap();
    crate::storage_usage::StorageWorkCounters::reset();
    let view = index.view().unwrap();
    let range = query(None, Some("@record.byte_length < 0"), 10, 0);
    assert!(ids(&view, &range).0.is_empty());
    let file_size = query(None, Some("@file.size < 0"), 10, 0);
    assert!(ids(&view, &file_size).0.is_empty());
    let counters = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(counters.record_row_cache_initializations, 0);
    assert_eq!(counters.file_row_cache_initializations, 0);
    assert_eq!(counters.catalog_row_cache_initializations, 0);

    let segment = view.segments(crate::model::CollectionId(0)).unwrap()[0].clone();
    assert!(segment.index.records.get(0).is_some());
    assert!(segment.index.records.cache_initialized());
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_compatibility_row_caches_initialize_once_and_are_shared_by_clones() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-compat-cache-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"compatibility"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    index.archive().unwrap();
    let view = index.view().unwrap();
    let segment = view.segments(crate::model::CollectionId(0)).unwrap()[0].clone();
    crate::storage_usage::StorageWorkCounters::reset();
    assert!(segment.index.files.get(0).is_some());
    assert!(segment.index.records.get(0).is_some());
    assert!(segment.index.files.get(0).is_some());
    assert!(segment.index.records.get(0).is_some());
    let counters = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(counters.file_row_cache_initializations, 1);
    assert_eq!(counters.record_row_cache_initializations, 1);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_storage_diagnostics_do_not_populate_caches_or_clone_payloads() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-storage-diagnostic-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"diagnostic payload"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let native_status = index.archive_status().unwrap();
    assert_eq!(native_status.archived_components, 0);
    let native_usage = index.view().unwrap().storage_usage();
    assert!(native_status.native_components > 0);
    assert_eq!(native_usage.archive.owners, 0);
    assert_eq!(native_usage.archive.serialized_bytes, 0);
    assert!(native_usage.posting_bitmap_payload_bytes > 0);
    index.archive().unwrap();
    let view = index.view().unwrap();
    crate::storage_usage::StorageWorkCounters::reset();
    let usage = view.storage_usage();
    let counters = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(counters.metadata_cache_initializations, 0);
    assert!(usage.archive.owners > 0);
    assert!(usage.archive.serialized_bytes > 0);
    assert!(usage.archive.allocated_bytes >= usage.archive.serialized_bytes);
    assert_eq!(usage.file_row_cache_bytes, 0);
    assert_eq!(usage.record_row_cache_bytes, 0);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_shared_archive_owner_is_counted_once_across_wrappers() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-shared-owner-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"shared owner"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let view = index.view().unwrap();
    let single = view.0.storage_usage();
    let mut duplicated = (*view.0).clone();
    let group = Arc::make_mut(&mut duplicated.segments[0]);
    group.push(Arc::clone(&group[0]));
    let double = duplicated.storage_usage();
    assert_eq!(double.archive.owners, single.archive.owners);
    assert_eq!(
        double.archive.serialized_bytes,
        single.archive.serialized_bytes
    );
    assert_eq!(
        double.archive.allocated_bytes,
        single.archive.allocated_bytes
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_repository_and_adopted_index_archive_accounting_match() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-accounting-adopt-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"accounting"}"#,
    )
    .unwrap();
    let repository =
        Repository::open_with_options(&root, vec![prototype_definition()], prototype_options())
            .unwrap();
    let before = repository.storage_usage();
    let index = Index::from_repository(repository).unwrap();
    let after = index.storage_usage().unwrap();
    assert!(after.archive.owners >= before.archive.owners);
    assert!(after.archive.owners <= before.archive.owners + 1);
    assert!(after.archive.serialized_bytes >= before.archive.serialized_bytes);
    assert!(after.archive.serialized_bytes <= before.archive.serialized_bytes + 64);
    assert_eq!(
        after.posting_bitmap_payload_bytes,
        before.posting_bitmap_payload_bytes
    );
    assert_eq!(after.bigram_payload_bytes, before.bigram_payload_bytes);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_pinned_view_releases_retired_segment_owner_when_dropped() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-pinned-owner-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/a.json");
    std::fs::write(&path, br#"{"id":"a","year":1,"title":"pinned old"}"#).unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let handle = index.view().unwrap().file_handle_for_path(&path).unwrap();
    let pinned = index.view().unwrap();
    let old = Arc::clone(
        pinned
            .segments(crate::model::CollectionId(0))
            .unwrap()
            .iter()
            .find(|segment| segment.index.file(handle.file_id).is_some())
            .unwrap(),
    );
    let weak = Arc::downgrade(&old.index);
    drop(old);
    std::fs::write(&path, br#"{"id":"a","year":2,"title":"pinned new"}"#).unwrap();
    index.update_file(handle, &path).unwrap();
    assert!(weak.upgrade().is_some());
    drop(pinned);
    for _ in 0..8 {
        if weak.upgrade().is_none() {
            break;
        }
        std::thread::yield_now();
    }
    assert!(
        weak.upgrade().is_none(),
        "retired segment archive remained owned"
    );
    assert_eq!(
        index
            .view()
            .unwrap()
            .search(&query(Some("pinned new"), None, 10, 0))
            .unwrap()
            .count,
        Some(1)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_archive_finalization_failure_publishes_nothing() {
    let root = std::env::temp_dir().join(format!(
        "objekte-rkyv-finalization-failure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":1,"title":"failure"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let before = index.view().unwrap();
    let revision = before.revision();
    let generation = index.archive_status().unwrap().representation_generation;
    let count = before.search(&query(None, None, 10, 0)).unwrap().count;
    drop(before);
    crate::archive_storage::fail_next_freeze();
    let error = index.archive().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected archive-finalization failure")
    );
    let view = index.view().unwrap();
    assert_eq!(view.revision(), revision);
    assert_eq!(
        index.archive_status().unwrap().representation_generation,
        generation
    );
    assert_eq!(view.search(&query(None, None, 10, 0)).unwrap().count, count);
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rkyv_deterministic_mutation_sequences_match_fresh_rebuilds_and_models() {
    let mut add_seen = false;
    let mut replace_seen = false;
    let mut delete_seen = false;
    let mut bounded_plain_seen = false;
    let mut bounded_merge_seen = false;
    let mut full_plain_seen = false;
    let mut full_merge_seen = false;
    for seed in [1u64, 7, 19, 41, 97] {
        for (build_workers, verified_cache_bytes) in [
            (1usize, 0usize),
            (1, 2 * 1024 * 1024),
            (3, 0),
            (3, 2 * 1024 * 1024),
        ] {
            let root = std::env::temp_dir().join(format!(
                "objekte-rkyv-seed-{seed}-{build_workers}-{verified_cache_bytes}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(root.join("data")).unwrap();
            let mut first = prototype_definition();
            first.name = "first".into();
            first.format = SourceFormat::JsonLines;
            let mut second = prototype_definition();
            second.name = "second".into();
            second.format = SourceFormat::JsonLines;
            let definitions = vec![first, second];
            let options = RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(build_workers),
                verified_cache_bytes,
                ..prototype_options()
            };
            let index = Index::build(&root, definitions.clone(), options).unwrap();
            let mut model = BTreeMap::<std::path::PathBuf, Vec<(String, i64)>>::new();
            let mut files = Vec::<(std::path::PathBuf, crate::lifecycle::FileHandle)>::new();
            let mut random = seed;
            let mut serial = 0usize;
            let mut file_serial = 0usize;

            for operation in 0..100 {
                let mut operation_description = String::from("compaction");
                let scheduled = match operation {
                    20 => Some((CompactionMode::Bounded, false)),
                    40 => Some((CompactionMode::Bounded, true)),
                    60 => Some((CompactionMode::Full, false)),
                    80 => Some((CompactionMode::Full, true)),
                    _ => None,
                };
                if let Some((mode, merge_small_segments)) = scheduled {
                    operation_description = format!("{mode:?} merge={merge_small_segments}");
                    index
                        .compact_with_options(CompactionOptions {
                            mode,
                            merge_small_segments,
                        })
                        .unwrap();
                    match (mode, merge_small_segments) {
                        (CompactionMode::Bounded, false) => bounded_plain_seen = true,
                        (CompactionMode::Bounded, true) => bounded_merge_seen = true,
                        (CompactionMode::Full, false) => full_plain_seen = true,
                        (CompactionMode::Full, true) => full_merge_seen = true,
                    }
                    assert_sequence_oracle(
                        &index,
                        &root,
                        &definitions,
                        &options,
                        &model,
                        seed,
                        operation,
                        &operation_description,
                    );
                    continue;
                }
                random = random
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let action = random % 100;
                if files.is_empty() || (action < 45 && files.len() < 24) {
                    let year = (random % 40) as i64;
                    let path = root.join(format!(
                        "data/file-{seed}-{build_workers}-{file_serial}.jsonl"
                    ));
                    operation_description = format!("add {}", path.display());
                    file_serial += 1;
                    let count = (random as usize % 13).min(12);
                    let mut records = Vec::with_capacity(count);
                    let mut body = String::new();
                    for _ in 0..count {
                        let id = format!("s{seed}_{serial}");
                        serial += 1;
                        let nullable = if random.is_multiple_of(3) {
                            ",\"maybe\":null".to_owned()
                        } else {
                            String::new()
                        };
                        body.push_str(&format!(
                        "{{\"id\":\"{id}\",\"year\":{year},\"title\":\"seed {seed} add token\",\"tags\":[\"a\",\"b\"]{nullable}}}\n"
                    ));
                        records.push((id, year));
                    }
                    std::fs::write(&path, body).unwrap();
                    let handle = index.add_file(&path).unwrap().file;
                    model.insert(path.clone(), records);
                    files.push((path, handle));
                    add_seen = true;
                } else if action < 75 {
                    let position = (random as usize) % files.len();
                    operation_description = format!("replace {}", files[position].0.display());
                    let year = (random % 40) as i64;
                    let count = (random as usize % 13).min(12);
                    let mut records = Vec::with_capacity(count);
                    let mut body = String::new();
                    for _ in 0..count {
                        let id = format!("s{seed}_{serial}");
                        serial += 1;
                        body.push_str(&format!(
                        "{{\"id\":\"{id}\",\"year\":{year},\"title\":\"seed {seed} replace token\"}}\n"
                    ));
                        records.push((id, year));
                    }
                    std::fs::write(&files[position].0, body).unwrap();
                    let handle = index
                        .update_file(files[position].1, &files[position].0)
                        .unwrap()
                        .file;
                    model.insert(files[position].0.clone(), records);
                    files[position].1 = handle;
                    replace_seen = true;
                } else if action < 94 {
                    let position = (random as usize) % files.len();
                    let (path, handle) = files.remove(position);
                    operation_description = format!("delete {}", path.display());
                    std::fs::remove_file(&path).unwrap();
                    index.delete_file(handle).unwrap();
                    model.remove(&path);
                    delete_seen = true;
                } else {
                    index
                        .compact_with_options(CompactionOptions {
                            mode: CompactionMode::Full,
                            merge_small_segments: true,
                        })
                        .unwrap();
                    full_merge_seen = true;
                }

                assert_sequence_oracle(
                    &index,
                    &root,
                    &definitions,
                    &options,
                    &model,
                    seed,
                    operation,
                    &operation_description,
                );
            }
            index
                .compact_with_options(CompactionOptions {
                    mode: CompactionMode::Full,
                    merge_small_segments: true,
                })
                .unwrap();
            assert_sequence_oracle(
                &index,
                &root,
                &definitions,
                &options,
                &model,
                seed,
                100,
                "final full compaction",
            );
            index.shutdown().unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    assert!(add_seen && replace_seen && delete_seen);
    assert!(bounded_plain_seen && bounded_merge_seen);
    assert!(full_plain_seen && full_merge_seen);
}

#[test]
fn explicit_archive_is_blocking_atomic_and_supports_mixed_lifecycle() {
    let root = std::env::temp_dir().join(format!(
        "objekte-explicit-archive-{}-{}",
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
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let native_view = index.view().unwrap();
    assert!(native_view.search_ready());
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Native
    );
    let native_ids = ids(&native_view, &query(None, None, 10, 0));
    let native_revision = native_view.revision();
    let native_file = native_view
        .file_handle_for_path(std::path::Path::new("data/a.json"))
        .unwrap();
    let pinned_weak =
        Arc::downgrade(&native_view.segments(crate::model::CollectionId(0)).unwrap()[0].index);
    let segment_id = native_view.segments(crate::model::CollectionId(0)).unwrap()[0].id;
    let native_segment = &native_view.segments(crate::model::CollectionId(0)).unwrap()[0].index;
    let bitmap_owner = native_segment.postings.fields[&vec!["year".into()]].value_bitmap_owner_id();
    let text_owner = Arc::as_ptr(&native_segment.raw) as usize;
    let base_text_owner = Arc::as_ptr(&native_view.repository().collections()[0].raw) as usize;
    let base_sort_owner = Arc::as_ptr(&native_view.repository().collections()[0].sorting) as usize;
    let base_bitmap_owner = native_view.repository().collections()[0].postings.fields
        [&vec!["year".into()]]
        .value_bitmap_owner_id();
    crate::storage_usage::StorageWorkCounters::reset();
    let report = index
        .archive_with_options(crate::lifecycle::ArchiveOptions::new(8).unwrap())
        .unwrap();
    assert_eq!(
        report.representation_after,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    assert!(report.converted_components > 0);
    assert!(report.newly_serialized_bytes > 0);
    assert!(report.peak_active_conversion_jobs <= 8);
    let archive_work = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(archive_work.temporary_ordinal_trees, 0);
    assert!(archive_work.checked_archive_validations > 0);
    assert_eq!(
        ids(&native_view, &query(None, None, 10, 0)),
        native_ids,
        "a pinned pre-archive view must continue using native storage"
    );
    drop(native_view);
    assert!(
        pinned_weak.upgrade().is_none(),
        "old native segment remained reachable after archive"
    );
    let archived_view = index.view().unwrap();
    assert_eq!(archived_view.revision(), native_revision);
    assert_eq!(
        archived_view
            .file_handle_for_path(std::path::Path::new("data/a.json"))
            .unwrap(),
        native_file
    );
    let archived_segment = archived_view
        .segments(crate::model::CollectionId(0))
        .unwrap()
        .iter()
        .find(|segment| segment.id == segment_id)
        .unwrap()
        .index
        .clone();
    assert_eq!(
        archived_segment.postings.fields[&vec!["year".into()]].value_bitmap_owner_id(),
        bitmap_owner,
        "archive conversion must retain the canonical bitmap payload allocation"
    );
    assert_eq!(
        Arc::as_ptr(&archived_segment.raw) as usize,
        text_owner,
        "archive conversion must retain native text payload owners"
    );
    assert_eq!(
        Arc::as_ptr(&archived_view.repository().collections()[0].raw) as usize,
        base_text_owner,
        "base text payload owners must survive archive conversion"
    );
    assert_eq!(
        Arc::as_ptr(&archived_view.repository().collections()[0].sorting) as usize,
        base_sort_owner,
        "base sorting payload owners must survive archive conversion"
    );
    assert_eq!(
        archived_view.repository().collections()[0].postings.fields[&vec!["year".into()]]
            .value_bitmap_owner_id(),
        base_bitmap_owner,
        "base posting bitmap owners must survive archive conversion"
    );
    assert_eq!(ids(&archived_view, &query(None, None, 10, 0)), native_ids);
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    let generation = index.archive_status().unwrap().representation_generation;
    let archive_usage = index.storage_usage().unwrap();
    let noop = index.archive().unwrap();
    assert_eq!(noop.converted_components, 0);
    assert_eq!(noop.representation_generation_after, generation);
    assert_eq!(noop.newly_serialized_bytes, 0);
    let noop_usage = index.storage_usage().unwrap();
    assert_eq!(noop_usage.archive.owners, archive_usage.archive.owners);
    assert_eq!(
        noop_usage.archive.serialized_bytes,
        archive_usage.archive.serialized_bytes
    );
    assert_eq!(
        noop_usage.archive.allocated_bytes,
        archive_usage.archive.allocated_bytes
    );
    drop(archived_view);

    let added_path = root.join("data/b.json");
    std::fs::write(&added_path, br#"{"id":"b","year":20,"title":"beta"}"#).unwrap();
    let added = index.add_file(&added_path).unwrap();
    assert_eq!(
        added.kind,
        crate::lifecycle_mutation::FileMutationKind::Added
    );
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Mixed
    );
    let mixed_view = index.view().unwrap();
    assert_eq!(ids(&mixed_view, &query(Some("beta"), None, 10, 0)).0, ["b"]);
    drop(mixed_view);
    let second = index.archive().unwrap();
    assert_eq!(
        second.representation_after,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    assert!(second.converted_components > 0);
    let archived_view = index.view().unwrap();
    assert_eq!(ids(&archived_view, &query(None, None, 10, 0)).0, ["a", "b"]);
    drop(archived_view);

    std::fs::write(
        &added_path,
        br#"{"id":"b","year":21,"title":"beta updated"}"#,
    )
    .unwrap();
    let updated = index.update_file(added.file, &added_path).unwrap();
    assert_eq!(updated.file.file_id, added.file.file_id);
    assert_eq!(
        ids(
            &index.view().unwrap(),
            &query(Some("beta updated"), None, 10, 0)
        )
        .0,
        ["b"]
    );
    index.archive().unwrap();
    let added = updated;
    index.delete_file(added.file).unwrap();
    assert_eq!(
        ids(&index.view().unwrap(), &query(None, None, 10, 0)).0,
        ["a"]
    );
    index.archive().unwrap();
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    index
        .compact_with_options(CompactionOptions {
            mode: CompactionMode::Full,
            merge_small_segments: true,
        })
        .unwrap();
    assert_eq!(
        ids(&index.view().unwrap(), &query(None, None, 10, 0)).0,
        ["a"]
    );
    index.archive().unwrap();
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_startup_and_empty_archive_have_explicit_boundaries() {
    let root = std::env::temp_dir().join(format!(
        "objekte-empty-native-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    crate::storage_usage::StorageWorkCounters::reset();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let startup = crate::storage_usage::StorageWorkCounters::snapshot();
    assert_eq!(startup.checked_archive_validations, 0);
    assert_eq!(startup.archive_bytes_generated, 0);
    let view = index.view().unwrap();
    assert!(view.search_ready());
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Native
    );
    drop(view);
    let report = index.archive().unwrap();
    assert!(report.converted_components > 0);
    assert!(report.newly_serialized_bytes > 0);
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    assert!(index.status().unwrap().search_ready);
    index.shutdown().unwrap();
    assert_eq!(
        index.archive().unwrap_err(),
        crate::lifecycle::IndexError::Cancelled
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_archive_calls_serialize_without_duplicate_conversion() {
    let root = std::env::temp_dir().join(format!(
        "objekte-concurrent-archive-{}-{}",
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
    let index =
        Arc::new(Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap());
    let left = Arc::clone(&index);
    let right = Arc::clone(&index);
    let left = std::thread::spawn(move || {
        left.archive_with_options(crate::lifecycle::ArchiveOptions::new(2).unwrap())
    });
    let right = std::thread::spawn(move || {
        right.archive_with_options(crate::lifecycle::ArchiveOptions::new(2).unwrap())
    });
    let left = left.join().unwrap().unwrap();
    let right = right.join().unwrap().unwrap();
    assert!(left.converted_components + right.converted_components > 0);
    assert_eq!(left.converted_components.min(right.converted_components), 0);
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_archive_failure_is_atomic_and_retryable() {
    let root = std::env::temp_dir().join(format!(
        "objekte-repository-archive-failure-{}-{}",
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
    let mut repository =
        Repository::open_with_options(&root, vec![prototype_definition()], prototype_options())
            .unwrap();
    let before = repository.archive_status();
    let count = repository.search(&query(None, None, 10, 0)).unwrap().count;
    crate::archive_storage::fail_next_freeze();
    assert!(repository.archive().is_err());
    assert_eq!(repository.archive_status(), before);
    assert_eq!(
        repository.search(&query(None, None, 10, 0)).unwrap().count,
        count
    );
    let report = repository.archive().unwrap();
    assert_eq!(
        report.representation_after,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    assert_eq!(
        repository.search(&query(None, None, 10, 0)).unwrap().count,
        count
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_scoped_worker_failures_leave_index_and_queries_unchanged() {
    use crate::archive_storage::{
        ArchiveFailureStage, ArchiveTestFaultControl, with_test_fault_control,
    };

    for stage in [
        ArchiveFailureStage::Serialization,
        ArchiveFailureStage::ByteValidation,
        ArchiveFailureStage::SemanticValidation,
        ArchiveFailureStage::FinalCandidateValidation,
        ArchiveFailureStage::PreInstall,
    ] {
        let root = std::env::temp_dir().join(format!(
            "objekte-index-stage-failure-{stage:?}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        for (id, year, title) in [("a", 10, "alpha"), ("b", 20, "beta")] {
            std::fs::write(
                root.join(format!("data/{id}.json")),
                format!(r#"{{"id":"{id}","year":{year},"title":"{title}"}}"#),
            )
            .unwrap();
        }
        let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
        let before_status = index.archive_status().unwrap();
        let before_revision = index.view().unwrap().revision();
        let before_all = ids(&index.view().unwrap(), &query(None, None, 10, 0));
        let before_old = ids(&index.view().unwrap(), &query(Some("alpha"), None, 10, 0));
        let successes = match stage {
            ArchiveFailureStage::Serialization | ArchiveFailureStage::ByteValidation => 2,
            ArchiveFailureStage::SemanticValidation
            | ArchiveFailureStage::FinalCandidateValidation
            | ArchiveFailureStage::PreInstall => 0,
        };
        crate::lifecycle::install_test_archive_fault_control(Arc::new(
            ArchiveTestFaultControl::new(stage, successes),
        ));
        let error = match index
            .archive_with_options(crate::lifecycle::ArchiveOptions::new(4).unwrap())
        {
            Err(error) => error,
            Ok(_) => panic!("{stage:?} fault was not injected"),
        };
        crate::lifecycle::clear_test_archive_fault_control();
        assert!(error.to_string().contains("injected"), "{stage:?}: {error}");
        assert_eq!(index.archive_status().unwrap(), before_status);
        let view = index.view().unwrap();
        assert_eq!(view.revision(), before_revision);
        assert_eq!(ids(&view, &query(None, None, 10, 0)), before_all);
        assert_eq!(ids(&view, &query(Some("alpha"), None, 10, 0)), before_old);
        drop(view);
        index.archive().unwrap();
        assert_eq!(
            index.archive_status().unwrap().representation,
            crate::lifecycle::ArchiveRepresentation::Archived
        );
        index.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    let root = std::env::temp_dir().join(format!(
        "objekte-posting-component-failure-{}-{}",
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
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let mut postings = index
        .view()
        .unwrap()
        .segments(crate::model::CollectionId(0))
        .unwrap()[0]
        .index
        .postings
        .clone();
    let error = with_test_fault_control(
        Arc::new(ArchiveTestFaultControl::new(
            ArchiveFailureStage::ByteValidation,
            1,
        )),
        || postings.archive_storage(),
    )
    .unwrap_err();
    assert!(error.contains("injected"));
    assert!(postings.all_native());
    assert_eq!(
        ids(&index.view().unwrap(), &query(Some("alpha"), None, 10, 0)).0,
        ["a"]
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn operation_scoped_worker_failure_leaves_repository_and_retry_unchanged() {
    use crate::archive_storage::{ArchiveFailureStage, ArchiveTestFaultControl};

    let root = std::env::temp_dir().join(format!(
        "objekte-repository-stage-failure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, year, title) in [("a", 10, "alpha"), ("b", 20, "beta")] {
        std::fs::write(
            root.join(format!("data/{id}.json")),
            format!(r#"{{"id":"{id}","year":{year},"title":"{title}"}}"#),
        )
        .unwrap();
    }
    let mut repository =
        Repository::open_with_options(&root, vec![prototype_definition()], prototype_options())
            .unwrap();
    let before_status = repository.archive_status();
    let before_all = repository.search(&query(None, None, 10, 0)).unwrap();
    let before_old = repository
        .search(&query(Some("alpha"), None, 10, 0))
        .unwrap();
    crate::lifecycle::install_test_archive_fault_control(Arc::new(ArchiveTestFaultControl::new(
        ArchiveFailureStage::Serialization,
        2,
    )));
    let error = repository
        .archive_with_options(crate::lifecycle::ArchiveOptions::new(4).unwrap())
        .unwrap_err();
    crate::lifecycle::clear_test_archive_fault_control();
    assert!(error.to_string().contains("injected"));
    assert_eq!(repository.archive_status(), before_status);
    assert_eq!(
        repository.search(&query(None, None, 10, 0)).unwrap().hits,
        before_all.hits
    );
    assert_eq!(
        repository
            .search(&query(Some("alpha"), None, 10, 0))
            .unwrap()
            .hits,
        before_old.hits
    );
    repository
        .archive_with_options(crate::lifecycle::ArchiveOptions::new(4).unwrap())
        .unwrap();
    assert_eq!(
        repository.archive_status().representation,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_collection_many_segments_archives_with_multiple_bounded_jobs() {
    let root = std::env::temp_dir().join(format!(
        "objekte-archive-segment-jobs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for ordinal in 0..8 {
        std::fs::write(
            root.join(format!("data/{ordinal}.json")),
            format!(r#"{{"id":"{ordinal}","year":{ordinal},"title":"job"}}"#),
        )
        .unwrap();
    }
    let index =
        Arc::new(Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap());
    let build_segments = index
        .view()
        .unwrap()
        .segments(crate::model::CollectionId(0))
        .unwrap()
        .len();
    assert!(build_segments >= 2);
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = Arc::clone(&index);
    let archive = std::thread::spawn(move || {
        let gate = crate::lifecycle::install_test_archive_job_gate();
        sender.send(Arc::clone(&gate)).unwrap();
        let result = worker.archive_with_options(crate::lifecycle::ArchiveOptions::new(4).unwrap());
        crate::lifecycle::clear_test_archive_job_gate();
        result
    });
    let gate = receiver.recv().unwrap();
    gate.wait_for_arrived(2);
    gate.release();
    let report = archive.join().unwrap().unwrap();
    assert!(report.peak_active_conversion_jobs >= 2);
    assert!(report.peak_active_conversion_jobs <= 4);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn archive_delete_archive_converts_the_current_catalog_without_resurrecting_records() {
    let root = std::env::temp_dir().join(format!(
        "objekte-archive-catalog-delete-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","year":10,"title":"keep alpha"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("data/b.json"),
        br#"{"id":"b","year":20,"title":"remove beta"}"#,
    )
    .unwrap();
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    index.archive().unwrap();
    let handle = index
        .view()
        .unwrap()
        .file_handle_for_path(&root.join("data/b.json"))
        .unwrap();
    index.delete_file(handle).unwrap();

    let before = index.archive_status().unwrap();
    assert_eq!(
        before.representation,
        crate::lifecycle::ArchiveRepresentation::Mixed
    );
    assert!(before.native_components > 0);
    let before_owners = index.storage_usage().unwrap().archive.owners;
    let report = index.archive().unwrap();
    assert!(report.converted_components > 0);
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Archived
    );
    assert!(index.storage_usage().unwrap().archive.owners >= before_owners);
    let view = index.view().unwrap();
    assert_eq!(ids(&view, &query(None, None, 10, 0)).0, ["a"]);
    assert!(
        ids(&view, &query(Some("remove beta"), None, 10, 0))
            .0
            .is_empty()
    );
    assert_eq!(ids(&view, &query(Some("keep alpha"), None, 10, 0)).0, ["a"]);
    drop(view);

    let generation = index.archive_status().unwrap().representation_generation;
    let noop = index.archive().unwrap();
    assert_eq!(noop.converted_components, 0);
    assert_eq!(
        index.archive_status().unwrap().representation_generation,
        generation
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queries_and_quiesce_coordinate_with_a_paused_archive() {
    let root = std::env::temp_dir().join(format!(
        "objekte-paused-archive-{}-{}",
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
    let index =
        Arc::new(Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap());
    let (gate_tx, gate_rx) = std::sync::mpsc::channel();
    let archive_index = Arc::clone(&index);
    let archive = std::thread::spawn(move || {
        let gate = crate::lifecycle::install_test_archive_gate();
        gate_tx.send(Arc::clone(&gate)).unwrap();
        let result = archive_index.archive();
        crate::lifecycle::clear_test_archive_gate();
        result
    });
    let gate = gate_rx.recv().unwrap();
    gate.wait_for_arrival();
    let pinned = index.view().unwrap();
    assert_eq!(
        ids(&pinned, &query(None, None, 10, 0)).0,
        ["a"],
        "queries must not wait for archive conversion"
    );
    drop(pinned);
    let generation = index.archive_status().unwrap().representation_generation;
    let quiesce_index = Arc::clone(&index);
    let quiesce = std::thread::spawn(move || quiesce_index.quiesce());
    gate.release();
    assert_eq!(
        archive.join().unwrap(),
        Err(crate::lifecycle::IndexError::Cancelled)
    );
    quiesce.join().unwrap().unwrap();
    assert_eq!(
        index.archive_status().unwrap().representation_generation,
        generation
    );
    assert_eq!(
        index.archive_status().unwrap().representation,
        crate::lifecycle::ArchiveRepresentation::Native
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[allow(clippy::too_many_arguments)]
fn assert_sequence_oracle(
    index: &Index,
    root: &std::path::Path,
    definitions: &[CollectionDefinition],
    options: &RepositoryOptions,
    model: &BTreeMap<std::path::PathBuf, Vec<(String, i64)>>,
    seed: u64,
    operation: usize,
    operation_description: &str,
) {
    let view = index.view().unwrap();
    for collection in 0..2 {
        let mut seen = std::collections::HashSet::new();
        for segment in view
            .segments(crate::model::CollectionId(collection))
            .unwrap()
            .iter()
        {
            for slot in 0..segment.active.slot_count() {
                let slot = crate::model::RecordSlot(slot);
                if !segment.active.contains(slot) {
                    continue;
                }
                let id = segment
                    .index
                    .record(slot)
                    .expect("active record row")
                    .id
                    .0
                    .to_string();
                assert!(
                    seen.insert(id.clone()),
                    "duplicate active logical ID {id} in collection {collection}, seed={seed}, operation={operation} ({operation_description})"
                );
            }
        }
    }
    let fresh = Index::build(root, definitions.to_vec(), *options).unwrap();
    let fresh_view = fresh.view().unwrap();
    let mut expected = model.values().flatten().collect::<Vec<_>>();
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
        let sorted = sequence_query(collection, None, 0, 1_000);
        let ge = sequence_query(collection, Some("year >= 15"), 0, 1_000);
        let (actual, count) = ids(&view, &sorted);
        let (fresh_actual, fresh_count) = ids(&fresh_view, &sorted);
        let failure_detail = if actual == expected_ids {
            String::new()
        } else {
            let missing = expected_ids
                .iter()
                .filter(|id| !actual.contains(id))
                .map(|id| {
                    let locations = (0..2)
                        .flat_map(|number| {
                            view.segments(crate::model::CollectionId(number))
                                .unwrap()
                                .iter()
                                .filter_map(move |segment| {
                                    let slot = segment.index.id_lookup.get(id)?;
                                    Some((
                                        number,
                                        crate::native_segment::SegmentId::new(segment.id.get()),
                                        slot,
                                        segment.active.contains(slot),
                                        segment.index.logical_records.contains(slot.0),
                                        segment
                                            .index
                                            .record(slot)
                                            .map(|record| record.id.0.to_string()),
                                    ))
                                })
                        })
                        .collect::<Vec<_>>();
                    format!("{id}:{locations:?}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(" missing={missing}")
        };
        assert_eq!(
            actual, expected_ids,
            "seed={seed} operation={operation} ({operation_description}) collection={collection}{failure_detail} model={model:?}"
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
            assert_eq!(
                ids(&view, &page),
                (vec![expected_ids[offset].clone()], Some(expected_ids.len()))
            );
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
                if segment
                    .active
                    .contains(crate::model::RecordSlot(slot as u32))
                {
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

fn sequence_query(collection: &str, filter: Option<&str>, offset: usize, limit: usize) -> Query {
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
    let index = Index::build(&root, vec![prototype_definition()], prototype_options()).unwrap();
    let request = query(Some("audit"), None, 10, 0);
    assert_eq!(
        index.view().unwrap().search(&request).unwrap().count,
        Some(2)
    );
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
            format!("{{\"id\":\"{number}\",\"year\":{number},\"title\":\"source-free\"}}"),
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
    let result = view.search(&query(None, Some("year >= 0"), 10, 0)).unwrap();
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
