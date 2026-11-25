use super::*;
use crate::model::LogicalType;
use crate::query::{CaseSensitivity, FtsMode, FtsQuery, QueryOptions, parse_filter};
use crate::repository::configuration::RepositoryOptions;
use crate::schema::FieldDefinition;
use crate::source::SourceFormat;

#[test]
fn source_scan_matches_reference_search_before_an_index_exists() {
    let root = std::env::temp_dir().join(format!(
        "objekte-source-scan-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/a.row"),
        br#"("title"="Alpha","meta"=("tag"="x"))"#,
    )
    .unwrap();
    std::fs::write(root.join("data/b.row"), br#"("title"="Beta")"#).unwrap();
    std::fs::write(root.join("data/c.row"), br#"("title"="broken""#).unwrap();
    let mut definition = crate::source::test_support::document_folder("files", "data");
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["meta".into(), "tag".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let scan = SourceScan::open(
        &root,
        vec![definition.clone()],
        crate::source::test_support::fixture_registry(),
    )
    .unwrap();
    let indexed = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            ..RepositoryOptions::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let mut query = Query {
        collections: vec!["files".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 1,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Bytes,
            evidence: true,
        },
    };
    let cancellation = CancellationToken::new();
    for (offset, fts, filter) in [
        (0, None, None),
        (1, None, None),
        (0, Some("Alpha"), None),
        (0, Some("broken"), None),
        (0, None, Some("meta.tag = 'x'")),
    ] {
        query.options.offset = offset;
        query.fts = fts.map(|literal| FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: literal.into(),
            pattern_parameter: None,
            fields: Vec::new(),
        });
        query.filter = filter.map(|expression| parse_filter(expression).unwrap());
        let expected = indexed.search_reference(&query).unwrap();
        let actual = scan.search(&query, &cancellation).unwrap();
        assert_eq!(
            actual.count, expected.count,
            "offset={offset} fts={fts:?} filter={filter:?}"
        );
        assert_eq!(
            actual.hits, expected.hits,
            "offset={offset} fts={fts:?} filter={filter:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Clone, Debug)]
struct ExpectedLocator {
    path: std::path::PathBuf,
    start: u64,
    end: u64,
    class: crate::inventory::RecordClass,
    id: RecordId,
}

fn locator_sequence(
    hits: &[crate::query_results::SearchHit],
) -> Vec<(
    String,
    std::path::PathBuf,
    u64,
    u64,
    crate::inventory::RecordClass,
    String,
)> {
    hits.iter()
        .map(|hit| {
            (
                hit.collection.clone(),
                hit.file.as_ref().expect("file origin").path.clone(),
                hit.source_range().expect("file range").start,
                hit.source_range().expect("file range").end,
                hit.record_class,
                hit.record_id.clone(),
            )
        })
        .collect()
}

fn assert_expected_locators(
    hits: &[crate::query_results::SearchHit],
    expected: &[ExpectedLocator],
    context: &str,
) {
    assert_eq!(hits.len(), expected.len(), "{context}");
    for row in expected {
        let matches = hits
            .iter()
            .filter(|hit| {
                hit.collection == "files"
                    && hit.file.as_ref().expect("file origin").path == row.path
                    && hit.source_range().expect("file range").start == row.start
                    && hit.source_range().expect("file range").end == row.end
                    && hit.record_class == row.class
                    && hit.record_id == row.id.0.as_ref()
            })
            .count();
        assert_eq!(matches, 1, "{context}: {row:?}");
    }
}

fn plain_query() -> Query {
    Query {
        collections: vec!["files".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 32,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

#[test]
fn relative_path_identity_checks_whole_file_ranges_and_fallback_ids() {
    let root = std::env::temp_dir().join(format!(
        "objekte-relative-whole-file-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    let plain = br#"("title"="plain")"#;
    let percent = br#"("title"="percent")"#;
    let broken = br#"("title"="broken""#;
    std::fs::write(root.join("data/plain.row"), plain).unwrap();
    std::fs::write(root.join("data/nested/percent%file.row"), percent).unwrap();
    std::fs::write(root.join("data/broken.row"), broken).unwrap();
    #[cfg(unix)]
    let backslash = br#"("title"="backslash")"#;
    #[cfg(unix)]
    std::fs::write(root.join("data/nested/back\\slash.row"), backslash).unwrap();

    for format in [
        crate::source::test_support::document_format(),
        crate::source::test_support::scalar_format(),
    ] {
        let mut definition = CollectionDefinition::folder("files", "data", format.clone());
        definition.identity = IdentityPolicy::RelativePath;
        let indexed = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let source = crate::source::test_support::fixture_registry()
            .resolve(&format)
            .unwrap();
        let file = |relative: &str| {
            let entry = indexed
                .files
                .files
                .iter()
                .find(|entry| entry.relative_path == std::path::Path::new(relative))
                .unwrap();
            (entry.id, entry.revision.modified_ns)
        };
        let (plain_file, plain_modified) = file("data/plain.row");
        let (percent_file, percent_modified) = file("data/nested/percent%file.row");
        let (broken_file, broken_modified) = file("data/broken.row");
        let relative_id = |relative: &std::path::Path| {
            RecordId::intrinsic(format!(
                "path:{}",
                crate::model::path_representation::query_path(relative)
            ))
            .unwrap()
        };
        let mut expected = vec![
            ExpectedLocator {
                path: "data/plain.row".into(),
                start: 0,
                end: plain.len() as u64,
                class: crate::inventory::RecordClass::Logical,
                id: relative_id(std::path::Path::new("plain.row")),
            },
            ExpectedLocator {
                path: "data/nested/percent%file.row".into(),
                start: 0,
                end: percent.len() as u64,
                class: crate::inventory::RecordClass::Logical,
                id: relative_id(std::path::Path::new("nested/percent%file.row")),
            },
            ExpectedLocator {
                path: "data/broken.row".into(),
                start: 0,
                end: broken.len() as u64,
                class: crate::inventory::RecordClass::RawFallback,
                id: RecordId::fallback(
                    CollectionId(0),
                    source.id(),
                    source.version(),
                    std::path::Path::new("data/broken.row"),
                    broken_file,
                    broken_modified,
                    0,
                    broken.len() as u64,
                ),
            },
        ];
        #[cfg(unix)]
        {
            let (backslash_file, backslash_modified) = file("data/nested/back\\slash.row");
            assert_eq!(
                relative_id(std::path::Path::new("nested/back\\slash.row"))
                    .0
                    .as_ref(),
                "path:nested/back%5Cslash.row"
            );
            expected.push(ExpectedLocator {
                path: "data/nested/back\\slash.row".into(),
                start: 0,
                end: backslash.len() as u64,
                class: crate::inventory::RecordClass::Logical,
                id: {
                    let id = relative_id(std::path::Path::new("nested/back\\slash.row"));
                    assert_eq!(id.0.as_ref(), "path:nested/back%5Cslash.row");
                    id
                },
            });
            let _ = (backslash_file, backslash_modified);
        }
        let _ = (plain_file, plain_modified, percent_file, percent_modified);

        let indexed_hits = indexed.search(&plain_query()).unwrap();
        let scan = SourceScan::open(
            &root,
            vec![definition],
            crate::source::test_support::fixture_registry(),
        )
        .unwrap();
        let scanned_hits = scan
            .search(&plain_query(), &CancellationToken::new())
            .unwrap();
        assert_eq!(indexed_hits.count, Some(expected.len()), "{format:?}");
        assert_eq!(scanned_hits.count, Some(expected.len()), "{format:?}");
        assert_expected_locators(&indexed_hits.hits, &expected, &format!("index {format:?}"));
        assert_expected_locators(
            &scanned_hits.hits,
            &expected,
            &format!("source scan {format:?}"),
        );
        assert_eq!(
            locator_sequence(&indexed_hits.hits),
            locator_sequence(&scanned_hits.hits),
            "{format:?}"
        );

        let mut logical = plain_query();
        logical.options.return_mode = ReturnMode::Logical;
        logical.options.evidence = true;
        for hits in [indexed.search(&logical).unwrap().hits, {
            scan.search(&logical, &CancellationToken::new())
                .unwrap()
                .hits
        }] {
            let fallback = hits
                .iter()
                .find(|hit| {
                    hit.file.as_ref().expect("file origin").path
                        == std::path::Path::new("data/broken.row")
                })
                .unwrap();
            assert!(matches!(
                &fallback.payload,
                Some(crate::query_results::Payload::LogicalUnavailable { diagnostic_ref })
                    if diagnostic_ref.stored_index().is_some()
            ));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn lines_identity_checks_exact_ranges_classes_and_fallback_identity() {
    let root = std::env::temp_dir().join(format!(
        "objekte-lines-identity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let first = br#"("title"="first")"#;
    let second = br#"("title"="second")"#;
    let broken = br#"("title"="broken""#;
    let bytes = [first.as_slice(), b"\n", second, b"\n", broken].concat();
    assert_eq!(bytes.len(), 54);
    std::fs::write(root.join("data/items.rows"), &bytes).unwrap();
    let definition =
        CollectionDefinition::folder("files", "data", crate::source::test_support::line_format());
    let indexed = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let file = indexed
        .files
        .files
        .iter()
        .find(|entry| entry.relative_path == std::path::Path::new("data/items.rows"))
        .unwrap();
    let expected = vec![
        ExpectedLocator {
            path: "data/items.rows".into(),
            start: 0,
            end: 17,
            class: crate::inventory::RecordClass::Logical,
            id: RecordId::generated(file.id, file.revision.modified_ns, 0),
        },
        ExpectedLocator {
            path: "data/items.rows".into(),
            start: 18,
            end: 36,
            class: crate::inventory::RecordClass::Logical,
            id: RecordId::generated(file.id, file.revision.modified_ns, 18),
        },
        ExpectedLocator {
            path: "data/items.rows".into(),
            start: 37,
            end: 54,
            class: crate::inventory::RecordClass::RawFallback,
            id: RecordId::fallback(
                CollectionId(0),
                "fixture-lines",
                1,
                std::path::Path::new("data/items.rows"),
                file.id,
                file.revision.modified_ns,
                37,
                54,
            ),
        },
    ];
    let indexed_hits = indexed.search(&plain_query()).unwrap();
    let scan = SourceScan::open(
        &root,
        vec![definition],
        crate::source::test_support::fixture_registry(),
    )
    .unwrap();
    let scanned_hits = scan
        .search(&plain_query(), &CancellationToken::new())
        .unwrap();
    assert_eq!(indexed_hits.count, Some(3));
    assert_eq!(scanned_hits.count, Some(3));
    assert_expected_locators(&indexed_hits.hits, &expected, "indexed line-framed source");
    assert_expected_locators(
        &scanned_hits.hits,
        &expected,
        "source-scan line-framed source",
    );
    assert_eq!(
        locator_sequence(&scanned_hits.hits),
        locator_sequence(&indexed_hits.hits)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registered_fixture_uses_generated_ids_with_declared_scalar_verification() {
    let root = std::env::temp_dir().join(format!(
        "objekte-registered-fixture-success-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let bytes = br#"("title"="registered")"#;
    std::fs::write(root.join("data/one.row"), bytes).unwrap();
    let format = SourceFormat::registered("fixture-record");
    let definition = CollectionDefinition::folder("files", "data", format.clone());
    let source = crate::source::test_support::fixture_registry()
        .resolve(&format)
        .unwrap();
    assert_eq!(
        source.scalar_verification(),
        Some(crate::source::verification::ScalarVerification::ResolvedStrings)
    );
    let indexed = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let file = indexed
        .files
        .files
        .iter()
        .find(|entry| entry.relative_path == std::path::Path::new("data/one.row"))
        .unwrap();
    let expected = vec![ExpectedLocator {
        path: "data/one.row".into(),
        start: 0,
        end: bytes.len() as u64,
        class: crate::inventory::RecordClass::Logical,
        id: RecordId::generated(file.id, file.revision.modified_ns, 0),
    }];
    let indexed_hits = indexed.search(&plain_query()).unwrap();
    let scan = SourceScan::open(
        &root,
        vec![definition],
        crate::source::test_support::fixture_registry(),
    )
    .unwrap();
    let scanned_hits = scan
        .search(&plain_query(), &CancellationToken::new())
        .unwrap();
    assert_expected_locators(&indexed_hits.hits, &expected, "indexed registered fixture");
    assert_expected_locators(
        &scanned_hits.hits,
        &expected,
        "source scan registered fixture",
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn relative_path_errors_name_one_valid_file_and_exact_range() {
    let root = std::env::temp_dir().join(format!(
        "objekte-relative-rejections-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("fixture-lines")).unwrap();
    std::fs::write(
        root.join("fixture-lines/valid.rows"),
        b"(\"title\"=\"valid\")\n",
    )
    .unwrap();
    for (format, source_path, file_name, end) in [(
        crate::source::test_support::line_format(),
        "fixture-lines",
        "valid.rows",
        17,
    )] {
        let mut definition = CollectionDefinition::folder("files", source_path, format.clone());
        definition.identity = IdentityPolicy::RelativePath;
        let error = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            crate::source::test_support::fixture_registry(),
        )
        .err()
        .expect("relative path should be rejected")
        .to_string();
        assert!(error.contains(file_name), "{format:?}: {error}");
        assert!(error.contains(&format!("0..{end}")), "{format:?}: {error}");
        assert!(
            error.contains("relative-path identity requires a reader that supports it"),
            "{format:?}: {error}"
        );
        let scan = SourceScan::open(
            &root,
            vec![definition],
            crate::source::test_support::fixture_registry(),
        )
        .unwrap();
        let error = scan
            .search(&plain_query(), &CancellationToken::new())
            .unwrap_err();
        assert_eq!(
            error,
            "relative-path identity requires a reader that supports it"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
