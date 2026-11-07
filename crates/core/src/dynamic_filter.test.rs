use super::*;
use crate::model::Value;
use crate::query::{Query, QueryOptions, parse_filter};
use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions, SnapshotMode};
use crate::source::SourceFormat;
use crate::workload::{BigramAlphabet, ReturnMode};

fn query(filter: &str) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 256,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

fn ids(result: &crate::repository::SearchResult) -> Vec<String> {
    result
        .hits
        .iter()
        .map(|hit| hit.record_id.clone())
        .collect()
}

fn fixture() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("objekte-dynamic-bigram-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, contents) in [
        ("a", r#"{"title":"Goethe","items":[{"title":"Goethe"}]}"#),
        ("b", r#"{"title":"goethe","items":[{"title":"Other"}]}"#),
        ("c", r#"{"title":"Other","noise":[]}"#),
        ("d", r#"{"title":[]}"#),
        ("e", r#"{"items":[]}"#),
        ("f", r#"{"title":42}"#),
        ("g", r#"{"title":null}"#),
        ("h", r#"{"other":"Goethe"}"#),
        ("i", r#"{"title":["Goethe","Other"]}"#),
        ("j", r#"{"title":["Goethe","Goethe"]}"#),
        ("k", r#"{"title":["Other","Goethe"]}"#),
        ("l", r#"{"title":""}"#),
        ("m", r#"{"title":"Müller"}"#),
        ("n", r#"{"title":"MÜLLER"}"#),
        ("o", r#"{"items":[{}]}"#),
        ("p", r#"{"title":["Goethe"],"items":[{"title":"Goethe"}]}"#),
        ("bad", "{"),
    ] {
        std::fs::write(root.join(format!("data/{id}.json")), contents).unwrap();
    }
    for number in 0..100 {
        std::fs::write(
            root.join(format!("data/z{number:03}.json")),
            r#"{"title":"unrelated"}"#,
        )
        .unwrap();
    }
    root
}

#[test]
fn dynamic_grams_preserve_empty_universal_truth_and_exact_results() {
    let root = fixture();
    let definition = || {
        let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
        definition.identity = IdentityPolicy::Filename;
        definition
    };
    let cases: &[(&str, &[&str])] = &[
        ("title ~ 'Goethe'", &["a", "d", "j", "p"]),
        ("title = 'Goethe'", &["a", "d", "j", "p"]),
        ("title ?~ 'Goethe'", &["a", "i", "j", "k", "p"]),
        ("title ?= 'Goethe'", &["a", "i", "j", "k", "p"]),
        ("title:lower ~ 'GOETHE'", &["a", "b", "d", "j", "p"]),
        ("items.title = 'Goethe'", &["a", "e", "p"]),
        ("items[*].title = 'Goethe'", &["a", "e", "p"]),
        ("items[0].title = 'Goethe'", &["a", "p"]),
        ("title ~ 'Müller'", &["d", "m"]),
        ("title:lower ~ 'MÜLLER'", &["d", "m", "n"]),
        ("title ~ 'ü'", &["d", "m"]),
        ("title ~ 'Goethe' && items[0].title ~ 'Goethe'", &["a", "p"]),
        (
            "title ~ 'Goethe' || title ~ 'Müller'",
            &["a", "d", "j", "m", "p"],
        ),
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            verified_cache_bytes: 1024 * 1024,
            build_workers: std::num::NonZeroUsize::new(2),
            ..Default::default()
        };
        for opening in 0..2 {
            let repo = Repository::open_with_options(&root, vec![definition()], options).unwrap();
            assert_eq!(repo.startup().restored, opening == 1);
            if opening == 0 {
                repo.save_default_snapshot_for_test();
            }
            let collection = &repo.collections[0];
            let empty_ids: Vec<_> = collection
                .empty_array_records
                .iter()
                .map(|slot| collection.generation.records[slot as usize].id.0.as_str())
                .collect();
            assert_eq!(empty_ids, ["c", "d", "e"]);
            assert_eq!(repo.build_parses, if opening == 0 { 116 } else { 0 });
            assert_eq!(repo.build_failed_parses, if opening == 0 { 1 } else { 0 });
            for &(expression, expected) in cases {
                let request = query(expression);
                let optimized = repo.search(&request).unwrap();
                let reference = repo.search_reference(&request).unwrap();
                assert_eq!(ids(&optimized), expected, "{alphabet:?} {expression}");
                assert_eq!(ids(&optimized), ids(&reference), "{expression}");
                assert_eq!(optimized.count, reference.count, "{expression}");
                assert_eq!(optimized.hits, reference.hits, "{expression}");
                assert_eq!(
                    ids(&repo.search(&request).unwrap()),
                    expected,
                    "warm {expression}"
                );
                let bound = crate::reference::bind_filter(
                    request.filter.as_ref().unwrap(),
                    &collection.definition.schema,
                )
                .unwrap();
                if let BoundExpr::Term(term) = bound
                    && let Some(candidates) = raw_bigram_candidates(collection, &term)
                {
                    for &id in expected {
                        let slot = collection
                            .id_lookup
                            .get(&crate::model::RecordId(id.into()))
                            .unwrap();
                        assert!(
                            candidates.contains(slot.0),
                            "candidate superset {expression}: {id}"
                        );
                    }
                }
            }
            let fallback_or = query("@record.class = 1 || title ~ 'Goethe'");
            let combined = repo.search(&fallback_or).unwrap();
            let reference = repo.search_reference(&fallback_or).unwrap();
            assert_eq!(combined.hits, reference.hits);
            assert_eq!(combined.count, Some(5));
            assert_eq!(
                combined
                    .hits
                    .iter()
                    .filter(|hit| hit.record_id.starts_with("~fallback:"))
                    .count(),
                1
            );
            for expression in ["!(title ~ 'Goethe')", "title ~ 'Go%the'"] {
                let request = query(expression);
                let result = repo.search(&request).unwrap();
                assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
                assert!(
                    result
                        .hits
                        .iter()
                        .all(|hit| !hit.record_id.starts_with("~fallback:"))
                );
            }
            let selective = query("title ?~ 'Goethe'");
            let plan = repo.explain(&selective).unwrap();
            let selected = &plan[0].terms[0];
            assert_eq!(selected.selected.kind, AccessPathKind::RawBigram);
            assert!(selected.dynamic_binding);
            assert!(selected.gram_candidates.unwrap() < 20);
            assert_eq!(selected.structural_candidates, None);
            assert!(selected.residual_verification);
            let cold = Repository::open_with_options(
                &root,
                vec![definition()],
                RepositoryOptions {
                    verified_cache_bytes: 0,
                    ..options
                },
            )
            .unwrap();
            let optimized = cold.search(&selective).unwrap();
            let reference = cold.search_reference(&selective).unwrap();
            assert!(optimized.counters.record_parses < reference.counters.record_parses);
            assert!(optimized.counters.files_opened < reference.counters.files_opened);
            let universal_plan = repo.explain(&query("title ~ 'Goethe'")).unwrap();
            let selected = &universal_plan[0].terms[0];
            assert_eq!(selected.selected.kind, AccessPathKind::RawBigram);
            assert_eq!(selected.structural_candidates, Some(3));
            assert!(selected.actual_output.is_none());
            let analyzed = repo.explain_analyze(&query("title ~ 'Goethe'")).unwrap().1;
            assert!(analyzed[0].terms[0].actual_output.unwrap() >= 4);
            for (expression, structural) in [
                ("items[0].title = 'Goethe'", None),
                ("items[*].title = 'Goethe'", Some(3)),
            ] {
                let plan = repo.explain(&query(expression)).unwrap();
                assert_eq!(plan[0].terms[0].selected.kind, AccessPathKind::RawBigram);
                assert_eq!(plan[0].terms[0].structural_candidates, structural);
            }
            let params =
                std::collections::HashMap::from([("title".into(), Value::String("Goethe".into()))]);
            let parameterized = query("title ?= $title");
            assert_eq!(
                ids(&repo
                    .search_with_parameters(&parameterized, &params)
                    .unwrap()),
                ["a", "i", "j", "k", "p"]
            );
            let changed =
                std::collections::HashMap::from([("title".into(), Value::String("Müller".into()))]);
            assert_eq!(
                ids(&repo
                    .search_with_parameters(&parameterized, &changed)
                    .unwrap()),
                ["m"]
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn broad_structural_superset_makes_scanning_cheaper() {
    let root = std::env::temp_dir().join(format!("objekte-dynamic-broad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for index in 0..64 {
        std::fs::write(
            root.join(format!("data/{index:03}.json")),
            format!(
                "{{\"title\":\"{}\",\"unrelated\":[]}}",
                if index == 0 { "needle" } else { "other" }
            ),
        )
        .unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Filename;
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(repo.collections[0].empty_array_records.len(), 64);
    let universal = query("title ~ 'needle'");
    let plan = repo.explain(&universal).unwrap();
    assert_eq!(plan[0].terms[0].selected.kind, AccessPathKind::Scanner);
    let answer = repo.search(&universal).unwrap();
    assert_eq!(ids(&answer), ["000"]);
    assert_eq!(answer.hits, repo.search_reference(&universal).unwrap().hits);
    let any = query("title ?~ 'needle'");
    assert_eq!(
        repo.explain(&any).unwrap()[0].terms[0].selected.kind,
        AccessPathKind::RawBigram
    );
    std::fs::remove_dir_all(root).unwrap();
}


/// A reader that may deliberately omit title from its raw text representation.
struct SelectedTextReader {
    complete: bool,
}


impl crate::source::SourceReader for SelectedTextReader {
    fn name(&self) -> &'static str {
        "selected-text"
    }

    fn logical_text_coverage(&self) -> crate::source::LogicalTextCoverage {
        if self.complete {
            crate::source::LogicalTextCoverage::AllStringFields
        } else {
            crate::source::LogicalTextCoverage::ReaderSelected
        }
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<crate::inventory::ByteRange>, String> {
        Ok(vec![crate::inventory::ByteRange::checked(
            0,
            file.len() as u64,
            file.len() as u64,
        )?])
    }

    fn parse(&self, bytes: &[u8]) -> Result<crate::source::ExtractedRecord, String> {
        let input = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let (title, body) = input.split_once('|').ok_or("missing field separator")?;
        let mut text = vec![crate::source::TextSegment {
            path: vec!["body".into()],
            text: body.into(),
        }];
        if self.complete {
            text.push(crate::source::TextSegment {
                path: vec!["title".into()],
                text: title.into(),
            });
        }
        Ok(crate::source::ExtractedRecord {
            value: Value::Object(std::collections::BTreeMap::from([
                ("title".into(), Value::String(title.into())),
                ("body".into(), Value::String(body.into())),
            ])),
            text,
        })
    }
}


#[test]
fn undeclared_candidate_respects_reader_text_coverage() {
    let root = std::env::temp_dir().join(format!("objekte-dynamic-reader-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/match.sel"), b"unique title|other body").unwrap();
    for index in 0..48 {
        std::fs::write(
            root.join(format!("data/{index:03}.sel")),
            format!("other {index}|body").as_bytes(),
        )
        .unwrap();
    }
    for complete in [false, true] {
        let mut sources = crate::source::SourceRegistry::builtin();
        sources
            .register(
                "selected-text",
                1,
                "sel",
                std::sync::Arc::new(SelectedTextReader { complete }),
            )
            .unwrap();
        let mut definition =
            CollectionDefinition::folder("data", "data", SourceFormat::registered("selected-text"));
        definition.identity = IdentityPolicy::Filename;
        let repo = Repository::open_with_sources(
            &root,
            vec![definition],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
            sources,
        )
        .unwrap();
        let request = query("title ?~ 'unique title'");
        let answer = repo.search(&request).unwrap();
        assert_eq!(ids(&answer), ["match"]);
        assert_eq!(answer.hits, repo.search_reference(&request).unwrap().hits);
        let plan = repo.explain(&request).unwrap();
        assert_eq!(
            plan[0].terms[0].selected.kind,
            if complete {
                AccessPathKind::RawBigram
            } else {
                AccessPathKind::Scanner
            }
        );
        if !complete {
            assert!(plan[0].terms[0].reason.contains("does not cover"));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
