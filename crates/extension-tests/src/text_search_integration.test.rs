use super::*;
use objekte_core::{
    CacheMode, Direction, JsonQuery, JsonQueryBackend, Options, Repository, Sort,
    TextFieldSelector, TextQuery, TextSearchOptions,
    extensions::{TextComparison, TextPreparation, TextSearchDefinition},
};
use std::path::Path;


/// Expand a manuscript abbreviation before matching or bigram extraction.
struct Abbreviations(Descriptor);


impl Default for Abbreviations {
    fn default() -> Self {
        Self(Descriptor::new(
            "example:expanded-abbreviations-values",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Text,
            DataKind::Text,
            vec![Dependency::SourceBytes],
        ))
    }
}


impl TextRepresentation for Abbreviations {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn binds(&self, context: &BindingContext<'_>) -> bool {
        matches!(context.path, "body" | "notes[]" | "notes[*]")
    }

    fn preparation(&self) -> TextPreparation {
        TextPreparation::Stored
    }

    fn render<'a>(&self, value: &'a str, output: &'a mut String) -> Result<Option<&'a str>> {
        if !value.contains("MS") {
            return Ok(Some(value));
        }
        output.clear();
        let mut rest = value;
        while let Some(index) = rest.find("MS") {
            output.push_str(&rest[..index]);
            output.push_str("manuscript");
            rest = &rest[index + 2..];
        }
        output.push_str(rest);
        Ok(Some(output))
    }
}


fn open(root: &Path, cache: CacheMode) -> Repository {
    open_with_budget(root, cache, 0)
}

fn open_with_budget(root: &Path, cache: CacheMode, budget: usize) -> Repository {
    let mut options = Options::new();
    options.cache = cache;
    options.query_cache_bytes = budget;
    let definition = TextSearchDefinition::new(
        Descriptor::new(
            "example:expanded-abbreviations",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Text,
            DataKind::Predicate,
            vec![Dependency::SourceBytes],
        ),
        "example:expanded-abbreviations-values",
    )
    .alias("abbreviations")
    .operator(TextComparison::Contains, "objekte:word-raw")
    .operator(TextComparison::Equal, "objekte:word-raw");
    Repository::builder(root)
        .options(options)
        .register_representation(Abbreviations::default())
        .unwrap()
        .register_text_definition(definition)
        .unwrap()
        .open()
        .unwrap()
}

#[test]
fn ordinary_text_query_uses_independent_representation_postings() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("data/docs");
    fs::create_dir_all(&folder).unwrap();
    let schema = root.path().join(".objekte/collections/docs.schema.json");
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(schema, r#"{"type":"object","properties":{"body":{"type":"string","x-text-search":{"default":"example:expanded-abbreviations","definitions":{"example:expanded-abbreviations":{"index":true},"objekte:markdown":{"index":true}}}},"title":{"type":"string"}},"x-objekte":{"name":"docs","path":"data/docs","type":"data","format":"json","id":"filename"}}"#).unwrap();
    for index in 0..128 {
        let text = match index {
            4 => "A rare MS witness",
            95 => "Another manuscript",
            _ => "Ordinary catalog entry",
        };
        fs::write(folder.join(format!("{index:03}.json")),
            serde_json::to_vec(&serde_json::json!({"body":text, "title": if index == 70 { "manuscript title" } else { "catalog" }})).unwrap()).unwrap();
    }
    let request = TextQuery {
        pattern: "manuscript".into(),
        collections: Some(vec!["docs".into()]),
        fields: Some(vec![TextFieldSelector::with_search(
            "body",
            "abbreviations",
        )]),
        count: true,
        ..Default::default()
    };
    let mut direct = open(root.path(), CacheMode::Disabled);
    let expected = direct.search_text(&request).unwrap();
    let mut indexed = open(root.path(), CacheMode::Memory);
    indexed
        .build_text_search_index("docs", "example:expanded-abbreviations", "body")
        .unwrap();
    let actual = indexed.search_text(&request).unwrap();
    let raw_conflict = TextQuery {
        mode: objekte_core::TextMode::WordRaw,
        fields: Some(vec![TextFieldSelector::new("body")]),
        ..request.clone()
    };
    assert!(
        indexed
            .search_text(&raw_conflict)
            .unwrap_err()
            .to_string()
            .contains("conflicts with raw-word")
    );
    indexed.warm_text_index("docs").unwrap();
    let regex = TextQuery {
        pattern: "manuscript".into(),
        mode: objekte_core::TextMode::Regex,
        fields: Some(vec![TextFieldSelector::with_search("body", "markdown")]),
        ..request.clone()
    };
    let expected_regex = direct.search_text(&regex).unwrap();
    let actual_regex = indexed.search_text(&regex).unwrap();
    assert_eq!(
        actual_regex
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>(),
        expected_regex
            .hits
            .iter()
            .map(|hit| &hit.record.id)
            .collect::<Vec<_>>()
    );
    assert!(
        actual_regex.metrics.selected_candidate_visits < 32,
        "{:?}",
        actual_regex.query_plan
    );
    let case_request = TextQuery {
        pattern: "MANUSCRIPT".into(),
        fields: Some(vec![TextFieldSelector::with_search("body", "markdown")]),
        search_options: TextSearchOptions {
            case_sensitive: Some(true),
            ..Default::default()
        },
        ..request.clone()
    };
    assert_eq!(indexed.search_text(&case_request).unwrap().total, Some(0));
    let mut target_override = case_request.clone();
    target_override.fields = Some(vec![
        TextFieldSelector::with_search("body", "markdown").with_options(TextSearchOptions {
            case_sensitive: Some(false),
            ..Default::default()
        }),
    ]);
    let encoded = serde_json::to_value(&target_override).unwrap();
    assert_eq!(encoded["fields"][0]["case_sensitive"], false);
    let target_override: TextQuery = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        indexed.search_text(&target_override).unwrap().total,
        Some(1)
    );
    let mut json_case = JsonQuery {
        filter: "body:markdown ~ 'MANUSCRIPT'".into(),
        count: true,
        ..Default::default()
    };
    json_case.fts.case_sensitive = true;
    assert_eq!(indexed.query_json(&json_case).unwrap().total, Some(0));
    json_case.fts.search_options.case_sensitive = Some(false);
    assert_eq!(indexed.query_json(&json_case).unwrap().total, Some(1));
    let ids = |result: &objekte_core::TextQueryResult| {
        result
            .hits
            .iter()
            .map(|hit| hit.record.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&actual), ids(&expected));
    assert_eq!(actual.total, expected.total);
    assert_eq!(actual.total, Some(2));
    assert_eq!(actual.hits[0].fields, ["body"]);
    assert!(
        actual.metrics.selected_candidate_visits < expected.metrics.selected_candidate_visits / 4
    );
    let mut unscoped = request.clone();
    unscoped.fields = None;
    let expected_unscoped = direct.search_text(&unscoped).unwrap();
    let actual_unscoped = indexed.search_text(&unscoped).unwrap();
    assert_eq!(ids(&actual_unscoped), ids(&expected_unscoped));
    assert_eq!(actual_unscoped.total, Some(3));
    assert!(
        actual_unscoped.metrics.selected_candidate_visits < 32,
        "unscoped plan={:?}",
        actual_unscoped.query_plan
    );
    let mut multi = request.clone();
    multi.fields = Some(vec![
        TextFieldSelector::with_search("body", "abbreviations"),
        TextFieldSelector::with_search("body", "markdown"),
    ]);
    let encoded = serde_json::to_value(&multi).unwrap();
    assert!(encoded["fields"][0].is_object());
    let multi: TextQuery = serde_json::from_value(encoded).unwrap();
    let expected_multi = direct.search_text(&multi).unwrap();
    let actual_multi = indexed.search_text(&multi).unwrap();
    assert_eq!(ids(&actual_multi), ids(&expected_multi));
    assert_eq!(actual_multi.total, Some(2));
    assert_eq!(
        actual_multi
            .hits
            .iter()
            .find(|hit| hit.record.id == "004")
            .unwrap()
            .matched_definitions[0]
            .definition,
        "example:expanded-abbreviations"
    );
    assert_eq!(
        actual_multi
            .hits
            .iter()
            .find(|hit| hit.record.id == "095")
            .unwrap()
            .matched_definitions
            .len(),
        2
    );

    let filter = "body:abbreviations ~ \"manuscript\"";
    let json_request = JsonQuery {
        filter: filter.into(),
        collections: Some(vec!["docs".into()]),
        count: true,
        backend: JsonQueryBackend::Auto,
        ..Default::default()
    };
    let json_direct = direct.query_json(&json_request).unwrap();
    let json_indexed = indexed.query_json(&json_request).unwrap();
    let json_ids = |result: &objekte_core::JsonQueryResult| {
        result
            .records
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(json_ids(&json_indexed), json_ids(&json_direct));
    assert_eq!(json_indexed.total, json_direct.total);
    assert!(
        json_indexed.metrics.selected_candidate_visits
            < json_direct.metrics.selected_candidate_visits / 4,
        "indexed={:?} direct={:?} plan={:?}",
        json_indexed.metrics.selected_candidate_visits,
        json_direct.metrics.selected_candidate_visits,
        json_indexed.query_plan
    );
    let mut text_warmed = open(root.path(), CacheMode::Memory);
    text_warmed.warm_text_index("docs").unwrap();
    let warmed_json = text_warmed.query_json(&json_request).unwrap();
    assert_eq!(json_ids(&warmed_json), json_ids(&json_direct));
    assert!(warmed_json.metrics.selected_candidate_visits < 32);
    for filter in [
        "body:fts ~ \"manuscript\"",
        "body:text(\"example:expanded-abbreviations\") ~ \"manuscript\"",
        "body:markdown ~ \"manuscript\"",
        "body:abbreviations = \"Another manuscript\"",
        "body:abbreviations !~ \"manuscript\"",
        "body:abbreviations ~ $term && @size > 0",
    ] {
        let mut query = json_request.clone();
        query.filter = filter.into();
        query
            .parameters
            .insert("term".into(), serde_json::json!("manuscript"));
        let expected = direct.query_json(&query).unwrap();
        let actual = indexed.query_json(&query).unwrap();
        assert_eq!(json_ids(&actual), json_ids(&expected), "{filter}");
        assert_eq!(actual.total, expected.total, "{filter}");
    }
    indexed.warm_bigram_index("docs").unwrap();
    indexed.save_bigram_cache("docs").unwrap();
    let mut restored = open(root.path(), CacheMode::Memory);
    restored.warm_bigram_index("docs").unwrap();
    let restored_result = restored.query_json(&json_request).unwrap();
    assert_eq!(json_ids(&restored_result), json_ids(&json_direct));
    assert!(restored_result.metrics.selected_candidate_visits < 32);
    let sorted = TextQuery {
        sort: vec![Sort {
            field: "@id".into(),
            direction: Direction::Asc,
        }],
        offset: 1,
        limit: 1,
        ..request.clone()
    };
    let expected_page = direct.search_text(&sorted).unwrap();
    let actual_page = restored.search_text(&sorted).unwrap();
    assert_eq!(ids(&actual_page), ids(&expected_page));
    assert_eq!(actual_page.total, expected_page.total);
    let mut streamed = Vec::new();
    let summary = restored
        .search_text_stream(&sorted, |_, hit| {
            streamed.push(hit.record.id.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(streamed, ids(&expected_page));
    assert_eq!(summary.total, expected_page.total);
    let mut cached = open_with_budget(root.path(), CacheMode::Memory, 64 * 1024 * 1024);
    cached.warm_bigram_index("docs").unwrap();
    let first = cached.search_text(&request).unwrap();
    let canonical = TextQuery {
        fields: Some(vec![TextFieldSelector::with_search(
            "body",
            "example:expanded-abbreviations",
        )]),
        offset: 1,
        limit: 1,
        ..request.clone()
    };
    let page = cached.search_text(&canonical).unwrap();
    assert_eq!(page.hits[0].record.id, first.hits[1].record.id);
    assert!(page.metrics.ordered_result_hits > 0);
    assert!(page.metrics.selected_descriptor_visits <= 2);
    fs::write(folder.join("004.json"), br#"{"body":"A repaired witness"}"#).unwrap();
    restored.refresh("docs").unwrap();
    let fresh = open(root.path(), CacheMode::Disabled)
        .query_json(&json_request)
        .unwrap();
    let refreshed = restored.query_json(&json_request).unwrap();
    assert_eq!(json_ids(&refreshed), json_ids(&fresh));
    assert_eq!(refreshed.total, fresh.total);
    fs::write(
        folder.join("129.json"),
        br#"{"body":"A late MS note","title":"catalog"}"#,
    )
    .unwrap();
    restored.rebuild_text_index("docs").unwrap();
    let rebuilt = restored.query_json(&json_request).unwrap();
    let oracle = open(root.path(), CacheMode::Disabled)
        .query_json(&json_request)
        .unwrap();
    assert_eq!(json_ids(&rebuilt), json_ids(&oracle));
    assert_eq!(rebuilt.total, oracle.total);
    restored.save_bigram_cache("docs").unwrap();
    let mut reopened = open(root.path(), CacheMode::Memory);
    reopened.warm_text_index("docs").unwrap();
    let reopened_result = reopened.query_json(&json_request).unwrap();
    assert_eq!(json_ids(&reopened_result), json_ids(&oracle));
    assert!(reopened_result.metrics.selected_candidate_visits < 32);
}

#[test]
fn explicit_unconfigured_text_build_keeps_cardinality_safety_and_candidates() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("data/docs");
    fs::create_dir_all(folder.join(".editor")).unwrap();
    fs::write(
        folder.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"body":{"type":"string"}}}"#,
    )
    .unwrap();
    for index in 0..128 {
        let body = if index == 77 {
            "rare MS witness"
        } else {
            "ordinary catalog"
        };
        fs::write(
            folder.join(format!("{index:03}.json")),
            serde_json::to_vec(&serde_json::json!({"body":body})).unwrap(),
        )
        .unwrap();
    }
    let request = JsonQuery {
        filter: "body:abbreviations ~ 'manuscript'".into(),
        count: true,
        ..Default::default()
    };
    let mut direct = open(root.path(), CacheMode::Disabled);
    let expected = direct.query_json(&request).unwrap();
    let plain = TextQuery {
        pattern: "RARE".into(),
        case_sensitive: true,
        fields: Some(vec![TextFieldSelector::new("body")]),
        count: true,
        ..Default::default()
    };
    assert_eq!(direct.search_text(&plain).unwrap().total, Some(0));
    let folded = TextQuery {
        search_options: TextSearchOptions {
            case_sensitive: Some(false),
            ..Default::default()
        },
        ..plain.clone()
    };
    assert_eq!(direct.search_text(&folded).unwrap().total, Some(1));
    let regex = TextQuery {
        pattern: "rare.*witness".into(),
        search_options: TextSearchOptions {
            syntax: Some(objekte_core::extensions::TextSyntax::Regex),
            ..Default::default()
        },
        ..plain
    };
    assert_eq!(direct.search_text(&regex).unwrap().total, Some(1));
    let mut indexed = open(root.path(), CacheMode::Memory);
    indexed
        .build_text_search_index("docs", "abbreviations", "body")
        .unwrap();
    let actual = indexed.query_json(&request).unwrap();
    assert_eq!(
        actual
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        expected
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(actual.total, Some(1));
    assert!(
        actual.metrics.selected_candidate_visits < 16,
        "{:?}",
        actual.query_plan
    );
    indexed.save_bigram_cache("docs").unwrap();
    let mut reopened = open(root.path(), CacheMode::Memory);
    reopened.warm_bigram_index("docs").unwrap();
    let restored = reopened.query_json(&request).unwrap();
    assert_eq!(
        restored
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        expected
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );
    assert!(
        restored.metrics.selected_candidate_visits < 16,
        "{:?}",
        restored.query_plan
    );
}

#[test]
fn mixed_sources_keep_registered_text_coverage() {
    let root = tempfile::tempdir().unwrap();
    let mixed = root.path().join("data/mixed");
    fs::create_dir_all(mixed.join(".editor")).unwrap();
    fs::write(mixed.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"body":{"type":"string","x-text-search":{"default":"example:expanded-abbreviations","definitions":{"example:expanded-abbreviations":{"index":true}}}}}}"#).unwrap();
    for index in 0..96 {
        let text = if index == 83 {
            "native MS witness"
        } else {
            "ordinary catalog"
        };
        fs::write(
            mixed.join(format!("n{index:03}.json")),
            serde_json::to_vec(&serde_json::json!({"body":text})).unwrap(),
        )
        .unwrap();
    }
    fs::write(mixed.join("z.md"), br#"{"body":"custom MS witness"}"#).unwrap();
    let make = |cache| {
        let mut options = Options::new();
        options.cache = cache;
        options.query_cache_bytes = 0;
        Repository::builder(root.path())
            .options(options)
            .register_source(super::review_stage1::JsonAliasProvider::new(mixed.clone()))
            .unwrap()
            .register_representation(Abbreviations::default())
            .unwrap()
            .register_text_definition(
                TextSearchDefinition::new(
                    Descriptor::new(
                        "example:expanded-abbreviations",
                        SemanticVersion {
                            major: 1,
                            minor: 0,
                            patch: 0,
                        },
                        &(),
                        DataKind::Text,
                        DataKind::Predicate,
                        vec![Dependency::SourceBytes],
                    ),
                    "example:expanded-abbreviations-values",
                )
                .alias("abbreviations")
                .operator(TextComparison::Contains, "objekte:word-raw")
                .operator(TextComparison::Equal, "objekte:word-raw"),
            )
            .unwrap()
            .open()
            .unwrap()
    };
    let mut indexed = make(CacheMode::Memory);
    indexed.warm_bigram_index("mixed").unwrap();
    let mut direct = make(CacheMode::Disabled);
    let request = TextQuery {
        pattern: "manuscript".into(),
        fields: Some(vec![TextFieldSelector::new("body")]),
        collections: Some(vec!["mixed".into()]),
        count: true,
        ..Default::default()
    };
    let expected = direct.search_text(&request).unwrap();
    let actual = indexed.search_text(&request).unwrap();
    let ids = |result: &objekte_core::TextQueryResult| {
        result
            .hits
            .iter()
            .map(|hit| hit.record.id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&actual), ids(&expected));
    assert_eq!(actual.total, Some(2));
    assert!(
        actual.metrics.selected_candidate_visits < 12,
        "{:?}",
        actual.query_plan
    );
    let json = JsonQuery {
        filter: "body:abbreviations ~ 'manuscript'".into(),
        collections: Some(vec!["mixed".into()]),
        count: true,
        ..Default::default()
    };
    let expected = direct.query_json(&json).unwrap();
    let actual = indexed.query_json(&json).unwrap();
    assert_eq!(
        actual
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>(),
        expected
            .records
            .iter()
            .map(|record| &record.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(actual.total, expected.total);
    assert!(
        actual.metrics.selected_candidate_visits < 12,
        "{:?}",
        actual.query_plan
    );
    fs::write(mixed.join("z.md"), b"{\"body\":\"manuscript\"").unwrap();
    indexed.refresh("mixed").unwrap();
    let mut repaired_oracle = make(CacheMode::Disabled);
    let malformed_scoped = indexed.search_text(&request).unwrap();
    let exact_scoped = repaired_oracle.search_text(&request).unwrap();
    assert_eq!(ids(&malformed_scoped), ids(&exact_scoped));
    assert!(!ids(&malformed_scoped).contains(&"z".to_owned()));
    let unscoped = TextQuery {
        fields: None,
        ..request.clone()
    };
    let malformed_raw = indexed.search_text(&unscoped).unwrap();
    assert!(
        malformed_raw
            .hits
            .iter()
            .any(|hit| hit.record.id == "z" && hit.fields == ["$raw"])
    );
    fs::write(mixed.join("z.md"), br#"{"body":"MS repaired"}"#).unwrap();
    indexed.refresh("mixed").unwrap();
    let repaired = indexed.search_text(&request).unwrap();
    let exact = make(CacheMode::Disabled).search_text(&request).unwrap();
    assert_eq!(ids(&repaired), ids(&exact));
    assert!(repaired.hits.iter().all(|hit| hit.fields != ["$raw"]));
}

#[test]
fn configured_array_text_path_keeps_concrete_match_locations() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("data/docs");
    fs::create_dir_all(folder.join(".editor")).unwrap();
    fs::write(folder.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"notes":{"type":"array","items":{"type":"string","x-text-search":{"default":"example:expanded-abbreviations","definitions":{"example:expanded-abbreviations":{"index":true}}}}}}}"#).unwrap();
    fs::write(
        folder.join("a.json"),
        br#"{"notes":["plain","MS witness"]}"#,
    )
    .unwrap();
    fs::write(folder.join("b.json"), br#"{"notes":["ordinary"]}"#).unwrap();
    let mut indexed = open(root.path(), CacheMode::Memory);
    indexed.warm_bigram_index("docs").unwrap();
    let mut direct = open(root.path(), CacheMode::Disabled);
    for path in ["notes[]", "notes[*]"] {
        let query = TextQuery {
            pattern: "manuscript".into(),
            fields: Some(vec![TextFieldSelector::with_search(path, "abbreviations")]),
            count: true,
            ..Default::default()
        };
        let expected = direct.search_text(&query).unwrap();
        let actual = indexed.search_text(&query).unwrap();
        assert_eq!(actual.total, expected.total);
        assert_eq!(actual.hits.len(), 1);
        assert_eq!(actual.hits[0].matched_paths, ["notes[1]"]);
        assert_eq!(actual.hits[0].matched_definitions[0].path, "notes[1]");
    }
}

#[test]
fn definitions_validate_aliases_components_and_ambiguous_defaults() {
    use objekte_core::extensions::{Extensions, NativeTextOperator};
    let definition = || {
        TextSearchDefinition::new(
            Descriptor::new(
                "example:expanded-abbreviations",
                SemanticVersion {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                &(),
                DataKind::Text,
                DataKind::Predicate,
                vec![Dependency::SourceBytes],
            ),
            "example:expanded-abbreviations-values",
        )
        .alias("abbreviations")
        .operator(TextComparison::Contains, "objekte:word-raw")
    };
    let mut first = Extensions::default();
    first
        .register_representation(Abbreviations::default())
        .unwrap();
    first
        .register_operator(NativeTextOperator::new(
            objekte_core::TextMode::WordRaw,
            false,
        ))
        .unwrap();
    first.register_text_definition(definition()).unwrap();
    let mut second = Extensions::default();
    second.register_text_definition(definition()).unwrap();
    second
        .register_operator(NativeTextOperator::new(
            objekte_core::TextMode::WordRaw,
            false,
        ))
        .unwrap();
    second
        .register_representation(Abbreviations::default())
        .unwrap();
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert!(second.register_text_definition(definition()).is_err());
    assert!(
        Extensions::default()
            .register_text_definition(definition().alias("length"))
            .is_err()
    );
    let root = tempfile::tempdir().unwrap();
    assert!(
        Repository::builder(root.path())
            .register_text_definition(definition())
            .unwrap()
            .open()
            .is_err()
    );

    let folder = root.path().join("data/docs");
    fs::create_dir_all(folder.join(".editor")).unwrap();
    fs::write(folder.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"body":{"type":"string","x-text-search":{"definitions":{"example:expanded-abbreviations":{"index":false},"objekte:markdown":{"index":false}}}}}}"#).unwrap();
    fs::write(folder.join("a.json"), br#"{"body":"MS witness"}"#).unwrap();
    let mut repo = open(root.path(), CacheMode::Disabled);
    let implicit = TextQuery {
        pattern: "manuscript".into(),
        fields: Some(vec!["body".into()]),
        ..Default::default()
    };
    assert!(repo.search_text(&implicit).is_err());
    let explicit = TextQuery {
        fields: Some(vec![TextFieldSelector::with_search(
            "body",
            "abbreviations",
        )]),
        ..implicit
    };
    assert_eq!(repo.search_text(&explicit).unwrap().hits.len(), 1);
    assert!(
        repo.query_json(&JsonQuery {
            filter: "body:unknown ~ 'x'".into(),
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn builtin_modifiers_share_legacy_markup_results() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("data/docs");
    fs::create_dir_all(folder.join(".editor")).unwrap();
    fs::write(folder.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"md":{"type":"string","x-full-text":"markdown"},"html":{"type":"string","x-full-text":"html"},"xml":{"type":"string","x-full-text":"xml"}}}"#).unwrap();
    fs::write(
        folder.join("a.json"),
        serde_json::to_vec(&serde_json::json!({
            "md":"**Goe**the, Johann",
            "html":"Goe<b>the</b>&amp; Johann",
            "xml":"<root>Goe<b>the</b> Johann</root>"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        folder.join("b.json"),
        br#"{"md":"Schiller","html":"<script>Goethe</script>Schiller","xml":"<root><bad"}"#,
    )
    .unwrap();
    let mut repo = Repository::open(root.path()).unwrap();
    for (path, definition) in [("md", "markdown"), ("html", "html"), ("xml", "xml")] {
        let legacy = JsonQuery {
            filter: format!("{path}:fts ~ 'Goethe Johann'"),
            count: true,
            ..Default::default()
        };
        let explicit = JsonQuery {
            filter: format!("{path}:{definition} ~ 'Goethe Johann'"),
            ..legacy.clone()
        };
        let old = repo.query_json(&legacy).unwrap();
        let new = repo.query_json(&explicit).unwrap();
        assert_eq!(
            new.records
                .iter()
                .map(|record| &record.id)
                .collect::<Vec<_>>(),
            old.records
                .iter()
                .map(|record| &record.id)
                .collect::<Vec<_>>(),
            "{path}"
        );
        assert_eq!(new.total, old.total, "{path}");
        let text = TextQuery {
            pattern: "Goethe Johann".into(),
            fields: Some(vec![TextFieldSelector::with_search(path, definition)]),
            count: true,
            ..Default::default()
        };
        let hit = repo.search_text(&text).unwrap();
        assert_eq!(
            hit.hits
                .iter()
                .map(|hit| &hit.record.id)
                .collect::<Vec<_>>(),
            vec!["a"],
            "{path}"
        );
    }
}


struct FallibleTextOperator(Descriptor);


impl Default for FallibleTextOperator {
    fn default() -> Self {
        Self(Descriptor::new(
            "example:fallible-text",
            SemanticVersion {
                major: 1,
                minor: 0,
                patch: 0,
            },
            &(),
            DataKind::Text,
            DataKind::Predicate,
            vec![Dependency::SourceBytes],
        ))
    }
}


struct FallibleTextPredicate;


impl CompiledOperator for FallibleTextPredicate {
    fn matches(&self, value: ScalarValue<'_>, _: &mut String) -> Result<bool> {
        let ScalarValue::Text(text) = value else {
            return Ok(false);
        };
        if text.contains("danger") {
            return Err(Error::Request("danger reached".into()));
        }
        Ok(text.contains("target"))
    }

    fn candidate(&self) -> CandidateConstraint {
        CandidateConstraint::BigramLiteral {
            literal: "target".into(),
            representation: "example:expanded-abbreviations-values".into(),
        }
    }
}


impl QueryOperator for FallibleTextOperator {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }

    fn compile(&self, _: &Value) -> Result<Box<dyn CompiledOperator>> {
        Ok(Box::new(FallibleTextPredicate))
    }
}


#[test]
fn ordinary_bound_text_preserves_operator_errors_and_boolean_order() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("data/docs");
    fs::create_dir_all(folder.join(".editor")).unwrap();
    fs::write(folder.join(".editor/schema.json"),
        r#"{"type":"object","properties":{"body":{"type":"string","x-text-search":{"default":"example:dangerous","definitions":{"example:dangerous":{"index":true}}}},"year":{"type":"integer","index":true}}}"#).unwrap();
    fs::write(folder.join("a.json"), br#"{"body":"danger","year":1850}"#).unwrap();
    fs::write(folder.join("b.json"), br#"{"body":"target","year":1848}"#).unwrap();
    let mut options = Options::new();
    options.query_cache_bytes = 64 * 1024 * 1024;
    let mut repo = Repository::builder(root.path())
        .options(options)
        .register_representation(Abbreviations::default())
        .unwrap()
        .register_operator(FallibleTextOperator::default())
        .unwrap()
        .register_text_definition(
            TextSearchDefinition::new(
                Descriptor::new(
                    "example:dangerous",
                    SemanticVersion {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    &(),
                    DataKind::Text,
                    DataKind::Predicate,
                    vec![Dependency::SourceBytes],
                ),
                "example:expanded-abbreviations-values",
            )
            .alias("dangerous")
            .operator(TextComparison::Contains, "example:fallible-text"),
        )
        .unwrap()
        .open()
        .unwrap();
    repo.warm_bigram_index("docs").unwrap();
    let text = TextQuery {
        pattern: "target".into(),
        fields: Some(vec![TextFieldSelector::with_search("body", "dangerous")]),
        ..Default::default()
    };
    assert!(
        repo.search_text(&text)
            .unwrap_err()
            .to_string()
            .contains("danger reached")
    );
    assert!(
        repo.search_text_stream(&text, |_, _| Ok(()))
            .unwrap_err()
            .to_string()
            .contains("danger reached")
    );
    for filter in [
        "body:dangerous ~ 'target'",
        "body:dangerous ~ 'target' && year = 1848",
        "body:dangerous ~ 'target' || year = 1850",
    ] {
        let query = JsonQuery {
            filter: filter.into(),
            ..Default::default()
        };
        assert!(
            repo.query_json(&query)
                .unwrap_err()
                .to_string()
                .contains("danger reached"),
            "{filter}"
        );
        assert!(
            repo.query_json(&query)
                .unwrap_err()
                .to_string()
                .contains("danger reached"),
            "{filter}"
        );
    }
    let safe = JsonQuery {
        filter: "year = 1850 || body:dangerous ~ 'target'".into(),
        ..Default::default()
    };
    assert_eq!(repo.query_json(&safe).unwrap().records.len(), 2);
}
