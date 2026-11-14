#[path = "support/legacy_storage.rs"]
mod legacy_storage;
use legacy_storage::*;
use std::collections::HashMap;

use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions,
};
use objekte_search_core::source::{SourceFormat, SourceRegistry};
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, WorkloadCase};
use serde_json::Value as JsonValue;

fn field(schema: &mut Schema, path: &[&str], kind: LogicalType, multi: bool) {
    schema
        .register_field(FieldDefinition {
            path: path.iter().map(|part| (*part).into()).collect(),
            kind,
            required: false,
            nullable: true,
            indexed: false,
            multi,
            element_nullable: vec![false],
        })
        .unwrap();
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

fn definitions() -> Vec<CollectionDefinition> {
    let mut entries = Schema::default();
    field(&mut entries, &["id"], LogicalType::Id, false);
    field(&mut entries, &["title"], LogicalType::String, false);
    let mut mentions = Schema::default();
    field(&mut mentions, &["id"], LogicalType::Id, false);
    field(
        &mut mentions,
        &["items"],
        LogicalType::Array(Box::new(LogicalType::Object)),
        true,
    );
    field(&mut mentions, &["items", "entry"], LogicalType::Id, true);
    field(
        &mut mentions,
        &["items", "local_title"],
        LogicalType::String,
        true,
    );
    field(
        &mut mentions,
        &["other"],
        LogicalType::Array(Box::new(LogicalType::Object)),
        true,
    );
    field(
        &mut mentions,
        &["other", "local_title"],
        LogicalType::String,
        true,
    );
    field(
        &mut mentions,
        &["groups"],
        LogicalType::Array(Box::new(LogicalType::Object)),
        true,
    );
    field(
        &mut mentions,
        &["groups", "group_title"],
        LogicalType::String,
        true,
    );
    field(
        &mut mentions,
        &["groups", "items"],
        LogicalType::Array(Box::new(LogicalType::Object)),
        true,
    );
    field(
        &mut mentions,
        &["groups", "items", "entry"],
        LogicalType::Id,
        true,
    );
    field(
        &mut mentions,
        &["groups", "items", "local_title"],
        LogicalType::String,
        true,
    );
    for path in [vec!["items", "entry"], vec!["groups", "items", "entry"]] {
        mentions
            .register_relation(RelationDefinition {
                path: path.into_iter().map(str::to_owned).collect(),
                target_collection: "entries".into(),
                cardinality: RelationCardinality::Many,
                reverse_alias: None,
            })
            .unwrap();
    }
    vec![
        definition("entries", entries),
        definition("mentions", mentions),
    ]
}

fn fixture() -> (
    std::path::PathBuf,
    Vec<JsonValue>,
    HashMap<String, JsonValue>,
) {
    let root = std::env::temp_dir().join(format!(
        "objekte-relation-occurrence-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for collection in ["entries", "mentions"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
    }
    let targets = [
        r#"{"id":"A","title":"Hymn"}"#,
        r#"{"id":"B","title":"Festival"}"#,
    ];
    let sources = [
        r#"{"id":"swapped","items":[{"entry":"A","local_title":"Festival"},{"entry":"B","local_title":"Hymn"}]}"#,
        r#"{"id":"paired","items":[{"entry":"A","local_title":"Hymn"},{"entry":"B","local_title":"Festival"}]}"#,
        r#"{"id":"mixed","items":[{"entry":"A","local_title":"Hymn"},{"entry":"B","local_title":"Other"}]}"#,
        r#"{"id":"dangling","items":[{"entry":"Z","local_title":"Hymn"}]}"#,
        r#"{"id":"mixed_dangling","items":[{"entry":"A","local_title":"Hymn"},{"entry":"Z","local_title":"Other"}]}"#,
        r#"{"id":"empty","items":[]}"#,
        r#"{"id":"missing"}"#,
        r#"{"id":"missing_sibling","items":[{"entry":"A"}]}"#,
        r#"{"id":"nested_swapped","groups":[{"items":[{"entry":"A","local_title":"Festival"}]},{"items":[{"entry":"B","local_title":"Hymn"}]}]}"#,
        r#"{"id":"nested_paired","groups":[{"group_title":"Hymn","items":[{"entry":"A","local_title":"Hymn"}]},{"group_title":"Festival","items":[{"entry":"B","local_title":"Festival"}]}]}"#,
        r#"{"id":"nested_first_only","groups":[{"items":[{"entry":"A","local_title":"Hymn"}]},{"items":[{"entry":"B","local_title":"Other"}]}]}"#,
    ];
    let mut target_values = HashMap::new();
    for (index, text) in targets.iter().enumerate() {
        std::fs::write(root.join(format!("entries/{index}.json")), text).unwrap();
        let value: JsonValue = serde_json::from_str(text).unwrap();
        target_values.insert(value["id"].as_str().unwrap().to_owned(), value);
    }
    let mut source_values = Vec::new();
    for (index, text) in sources.iter().enumerate() {
        std::fs::write(root.join(format!("mentions/{index}.json")), text).unwrap();
        source_values.push(serde_json::from_str(text).unwrap());
    }
    (root, source_values, target_values)
}

// This oracle reads fixture JSON directly. It never uses bound paths,
// adjacency, postings, scanner evaluation, or the relation comparator.
fn oracle_item_pairs(
    source: &JsonValue,
    targets: &HashMap<String, JsonValue>,
    nested: bool,
    group_index: Option<usize>,
    existential: bool,
) -> Option<bool> {
    let Some(groups) = (if nested {
        source.get("groups").and_then(JsonValue::as_array)
    } else {
        source.get("items").and_then(JsonValue::as_array)
    }) else {
        return Some(false);
    };
    let mut pairs = Vec::new();
    if nested {
        for (index, group) in groups.iter().enumerate() {
            if group_index.is_some_and(|selected| selected != index) {
                continue;
            }
            if let Some(items) = group.get("items").and_then(JsonValue::as_array) {
                for item in items {
                    pairs.push(item);
                }
            }
        }
    } else {
        pairs.extend(groups);
    }
    let mut unknown = false;
    for item in pairs {
        let answer = item
            .get("entry")
            .and_then(JsonValue::as_str)
            .and_then(|id| targets.get(id))
            .map(|target| {
                item.get("local_title").and_then(JsonValue::as_str)
                    == target.get("title").and_then(JsonValue::as_str)
            });
        match (existential, answer) {
            (true, Some(true)) => return Some(true),
            (false, Some(false)) => return Some(false),
            (_, None) => unknown = true,
            _ => {}
        }
    }
    if unknown { None } else { Some(!existential) }
}

fn query(filter: &str, mode: ReturnMode) -> Query {
    Query::from_workload(&WorkloadCase {
        id: "occurrence".into(),
        collection: "mentions".into(),
        fts: None,
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        filter: Some(filter.into()),
        fts_fields: vec![],
        sort: vec![],
        limit: 100,
        offset: 0,
        count: true,
        return_mode: mode,
        evidence: true,
    })
    .unwrap()
}

#[test]
fn shared_source_occurrences_match_independent_oracle_across_routes_and_restore() {
    let (root, sources, targets) = fixture();
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            verified_cache_bytes: 1 << 20,
            ..Default::default()
        };
        let built = Repository::open_with_options(&root, definitions(), options).unwrap();
        let policy = IndexStoragePolicy {
            location: IndexStorageLocation::RepositoryDirectory,
        };
        let first = Index::open_with_storage(
            &root,
            definitions(),
            options,
            policy.clone(),
            SourceRegistry::builtin(),
        )
        .unwrap();
        first.stop_sync_storage().unwrap();
        let restored = Index::open_with_storage(
            &root,
            definitions(),
            options,
            policy,
            SourceRegistry::builtin(),
        )
        .unwrap();
        assert!(restored.restored_from_storage());
        let view = restored.view().unwrap();
        for stored in [false, true] {
            let search = |query: &Query| {
                if stored {
                    view.search(query)
                } else {
                    built.search(query)
                }
            };
            for (filter, nested, group_index, existential, negated, known) in [
                (
                    "items[*].local_title = items[*].entry.title",
                    false,
                    None,
                    false,
                    false,
                    vec!["empty", "paired"],
                ),
                (
                    "items[*].entry.title ?= items[*].local_title",
                    false,
                    None,
                    true,
                    false,
                    vec!["mixed", "mixed_dangling", "paired"],
                ),
                (
                    "!(items[*].local_title = items[*].entry.title)",
                    false,
                    None,
                    false,
                    true,
                    vec![
                        "missing",
                        "missing_sibling",
                        "mixed",
                        "nested_first_only",
                        "nested_paired",
                        "nested_swapped",
                        "swapped",
                    ],
                ),
                (
                    "groups[*].items[*].local_title ?= groups[*].items[*].entry.title",
                    true,
                    None,
                    true,
                    false,
                    vec!["nested_first_only", "nested_paired"],
                ),
                (
                    "groups[1].items[*].local_title ?= groups[1].items[*].entry.title",
                    true,
                    Some(1),
                    true,
                    false,
                    vec!["nested_paired"],
                ),
            ] {
                let mut expected = sources
                    .iter()
                    .filter(|source| {
                        let answer =
                            oracle_item_pairs(source, &targets, nested, group_index, existential);
                        (if negated {
                            answer.map(|value| !value)
                        } else {
                            answer
                        }) == Some(true)
                    })
                    .map(|source| source["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>();
                expected.sort();
                assert_eq!(expected, known, "independent fixture oracle for {filter}");
                for mode in [ReturnMode::Locator, ReturnMode::Bytes, ReturnMode::Logical] {
                    let request = query(filter, mode);
                    for _ in 0..2 {
                        let optimized = search(&request).unwrap();
                        let reference = built.search_reference(&request).unwrap();
                        let mut actual = optimized
                            .hits
                            .iter()
                            .map(|hit| hit.record_id.clone())
                            .collect::<Vec<_>>();
                        actual.sort();
                        assert_eq!(actual, expected, "{filter}");
                        assert_eq!(optimized.count, Some(expected.len()), "{filter}");
                        assert_eq!(optimized.hits, reference.hits, "{filter}");
                        assert_eq!(optimized.count, reference.count, "{filter}");
                    }
                }
            }
            for filter in [
                "items[*].local_title = groups[*].items[*].entry.title",
                "items[*].local_title = items.entry.title",
                "other[*].local_title = items[*].entry.title",
            ] {
                assert!(
                    search(&query(filter, ReturnMode::Locator))
                        .unwrap_err()
                        .contains("shared [*]"),
                    "{filter}"
                );
            }
        }
        restored.stop_sync_storage().unwrap();
        restored.wait_for_reconciliation().unwrap();
    }
}
