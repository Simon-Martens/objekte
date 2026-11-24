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
        root.join("data/a.json"),
        br#"{"title":"Alpha","meta":{"tag":"x"}}"#,
    )
    .unwrap();
    std::fs::write(root.join("data/b.json"), br#"{"title":"Beta"}"#).unwrap();
    std::fs::write(root.join("data/c.json"), br#"{"title":"broken""#).unwrap();
    let mut definition = CollectionDefinition::folder("files", "data", SourceFormat::Json);
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
    let scan = SourceScan::open(&root, vec![definition.clone()]).unwrap();
    let indexed = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions {
            ..RepositoryOptions::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
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
    let plain = br#"{"title":"plain"}"#;
    let percent = br#"{"title":"percent"}"#;
    let broken = br#"{"title":"broken""#;
    std::fs::write(root.join("data/plain.json"), plain).unwrap();
    std::fs::write(root.join("data/nested/percent%file.json"), percent).unwrap();
    std::fs::write(root.join("data/broken.json"), broken).unwrap();
    #[cfg(unix)]
    let backslash = br#"{"title":"backslash"}"#;
    #[cfg(unix)]
    std::fs::write(root.join("data/nested/back\\slash.json"), backslash).unwrap();

    for format in [SourceFormat::Json, SourceFormat::JsonDecodedScalars] {
        let mut definition = CollectionDefinition::folder("files", "data", format.clone());
        definition.identity = IdentityPolicy::RelativePath;
        let indexed = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let source = SourceRegistry::builtin().resolve(&format).unwrap();
        let file = |relative: &str| {
            let entry = indexed
                .files
                .files
                .iter()
                .find(|entry| entry.relative_path == std::path::Path::new(relative))
                .unwrap();
            (entry.id, entry.revision.modified_ns)
        };
        let (plain_file, plain_modified) = file("data/plain.json");
        let (percent_file, percent_modified) = file("data/nested/percent%file.json");
        let (broken_file, broken_modified) = file("data/broken.json");
        let relative_id = |relative: &std::path::Path| {
            RecordId::intrinsic(format!(
                "path:{}",
                crate::model::path_representation::query_path(relative)
            ))
            .unwrap()
        };
        let mut expected = vec![
            ExpectedLocator {
                path: "data/plain.json".into(),
                start: 0,
                end: plain.len() as u64,
                class: crate::inventory::RecordClass::Logical,
                id: relative_id(std::path::Path::new("plain.json")),
            },
            ExpectedLocator {
                path: "data/nested/percent%file.json".into(),
                start: 0,
                end: percent.len() as u64,
                class: crate::inventory::RecordClass::Logical,
                id: relative_id(std::path::Path::new("nested/percent%file.json")),
            },
            ExpectedLocator {
                path: "data/broken.json".into(),
                start: 0,
                end: broken.len() as u64,
                class: crate::inventory::RecordClass::RawFallback,
                id: RecordId::fallback(
                    CollectionId(0),
                    source.id(),
                    source.version(),
                    std::path::Path::new("data/broken.json"),
                    broken_file,
                    broken_modified,
                    0,
                    broken.len() as u64,
                ),
            },
        ];
        #[cfg(unix)]
        {
            let (backslash_file, backslash_modified) = file("data/nested/back\\slash.json");
            assert_eq!(
                relative_id(std::path::Path::new("nested/back\\slash.json"))
                    .0
                    .as_ref(),
                "path:nested/back%5Cslash.json"
            );
            expected.push(ExpectedLocator {
                path: "data/nested/back\\slash.json".into(),
                start: 0,
                end: backslash.len() as u64,
                class: crate::inventory::RecordClass::Logical,
                id: {
                    let id = relative_id(std::path::Path::new("nested/back\\slash.json"));
                    assert_eq!(id.0.as_ref(), "path:nested/back%5Cslash.json");
                    id
                },
            });
            let _ = (backslash_file, backslash_modified);
        }
        let _ = (plain_file, plain_modified, percent_file, percent_modified);

        let indexed_hits = indexed.search(&plain_query()).unwrap();
        let scan = SourceScan::open(&root, vec![definition]).unwrap();
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
                        == std::path::Path::new("data/broken.json")
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
fn jsonl_identity_checks_exact_ranges_classes_and_fallback_identity() {
    let root = std::env::temp_dir().join(format!(
        "objekte-jsonl-identity-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let first = br#"{"title":"first"}"#;
    let second = br#"{"title":"second"}"#;
    let broken = br#"{"title":"broken""#;
    let bytes = [first.as_slice(), b"\n", second, b"\n", broken].concat();
    assert_eq!(bytes.len(), 54);
    std::fs::write(root.join("data/items.jsonl"), &bytes).unwrap();
    let definition = CollectionDefinition::folder("files", "data", SourceFormat::JsonLines);
    let indexed = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let file = indexed
        .files
        .files
        .iter()
        .find(|entry| entry.relative_path == std::path::Path::new("data/items.jsonl"))
        .unwrap();
    let expected = vec![
        ExpectedLocator {
            path: "data/items.jsonl".into(),
            start: 0,
            end: 17,
            class: crate::inventory::RecordClass::Logical,
            id: RecordId::generated(file.id, file.revision.modified_ns, 0),
        },
        ExpectedLocator {
            path: "data/items.jsonl".into(),
            start: 18,
            end: 36,
            class: crate::inventory::RecordClass::Logical,
            id: RecordId::generated(file.id, file.revision.modified_ns, 18),
        },
        ExpectedLocator {
            path: "data/items.jsonl".into(),
            start: 37,
            end: 54,
            class: crate::inventory::RecordClass::RawFallback,
            id: RecordId::fallback(
                CollectionId(0),
                "jsonl",
                1,
                std::path::Path::new("data/items.jsonl"),
                file.id,
                file.revision.modified_ns,
                37,
                54,
            ),
        },
    ];
    let indexed_hits = indexed.search(&plain_query()).unwrap();
    let scan = SourceScan::open(&root, vec![definition]).unwrap();
    let scanned_hits = scan
        .search(&plain_query(), &CancellationToken::new())
        .unwrap();
    assert_eq!(indexed_hits.count, Some(3));
    assert_eq!(scanned_hits.count, Some(3));
    assert_expected_locators(&indexed_hits.hits, &expected, "indexed JSONL");
    assert_expected_locators(&scanned_hits.hits, &expected, "source-scan JSONL");
    assert_eq!(
        locator_sequence(&scanned_hits.hits),
        locator_sequence(&indexed_hits.hits)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn registered_json_uses_generated_ids_with_builtin_scalar_verification() {
    let root = std::env::temp_dir().join(format!(
        "objekte-registered-json-success-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let bytes = br#"{"title":"registered"}"#;
    std::fs::write(root.join("data/one.json"), bytes).unwrap();
    let format = SourceFormat::registered("json");
    let definition = CollectionDefinition::folder("files", "data", format.clone());
    let source = SourceRegistry::builtin().resolve(&format).unwrap();
    assert_eq!(
        source.scalar_verification(),
        Some(crate::source::verification::ScalarVerification::ResolvedStrings)
    );
    let indexed = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let file = indexed
        .files
        .files
        .iter()
        .find(|entry| entry.relative_path == std::path::Path::new("data/one.json"))
        .unwrap();
    let expected = vec![ExpectedLocator {
        path: "data/one.json".into(),
        start: 0,
        end: bytes.len() as u64,
        class: crate::inventory::RecordClass::Logical,
        id: RecordId::generated(file.id, file.revision.modified_ns, 0),
    }];
    let indexed_hits = indexed.search(&plain_query()).unwrap();
    let scan = SourceScan::open(&root, vec![definition]).unwrap();
    let scanned_hits = scan
        .search(&plain_query(), &CancellationToken::new())
        .unwrap();
    assert_expected_locators(&indexed_hits.hits, &expected, "indexed registered json");
    assert_expected_locators(&scanned_hits.hits, &expected, "source scan registered json");
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
    std::fs::create_dir_all(root.join("jsonl")).unwrap();
    std::fs::create_dir_all(root.join("registered")).unwrap();
    std::fs::write(root.join("jsonl/valid.jsonl"), b"{\"title\":\"valid\"}\n").unwrap();
    std::fs::write(root.join("registered/valid.json"), br#"{"title":"valid"}"#).unwrap();
    for (format, source_path, file_name, end) in [
        (SourceFormat::JsonLines, "jsonl", "valid.jsonl", 17),
        (
            SourceFormat::registered("json"),
            "registered",
            "valid.json",
            17,
        ),
    ] {
        let mut definition = CollectionDefinition::folder("files", source_path, format.clone());
        definition.identity = IdentityPolicy::RelativePath;
        let error = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            SourceRegistry::builtin(),
        )
        .err()
        .expect("relative path should be rejected")
        .to_string();
        assert!(error.contains(file_name), "{format:?}: {error}");
        assert!(error.contains(&format!("0..{end}")), "{format:?}: {error}");
        assert!(
            error.contains("relative-path identity requires a single-record JSON reader"),
            "{format:?}: {error}"
        );
        let scan = SourceScan::open(&root, vec![definition]).unwrap();
        let error = scan
            .search(&plain_query(), &CancellationToken::new())
            .unwrap_err();
        assert_eq!(
            error,
            "relative-path identity requires a single-record JSON reader"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
