use super::*;

use crate::model::LogicalType;
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions, SnapshotMode};
use crate::source::SourceFormat;
use crate::workload::BigramAlphabet;

#[test]
fn typed_posting_values_count_distinct_records_without_source_reads() {
    let root = std::env::temp_dir().join(format!("objekte-posting-values-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/multi.jsonl");
    std::fs::write(&path, b"{\"tags\":[\"a\",\"a\",\"b\"]}\n{\"tags\":[\"a\",\"c\"]}\n{\"tags\":null}\n{}\n{\"tags\":\n").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["tags".into()],
            kind: LogicalType::Array(Box::new(LogicalType::String)),
            required: false,
            nullable: true,
            indexed: true,
            multi: true,
            element_nullable: vec![],
        })
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::IfValid,
            ..Default::default()
        };
        let repo = Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        let mut request = PostingValuesRequest::new("data", "tags", 2);
        let first = repo.posting_values(&request).unwrap();
        assert_eq!(
            first.values,
            [
                PostingValueCount {
                    value: ScalarKey::String("a".into()),
                    count: 2
                },
                PostingValueCount {
                    value: ScalarKey::String("b".into()),
                    count: 1
                }
            ]
        );
        assert_eq!(
            first.coverage,
            PostingValuesCoverage {
                eligible_records: 4,
                unavailable_records: 1,
                selected_records: None,
                present_records: 3,
                explicit_null_records: 1
            }
        );
        assert_eq!(first.visited_keys, 3);
        request.continuation = first.continuation;
        let second = repo.posting_values(&request).unwrap();
        assert_eq!(
            second.values,
            [PostingValueCount {
                value: ScalarKey::String("c".into()),
                count: 1
            }]
        );
        assert!(second.continuation.is_none());
        request.descending = true;
        request.continuation = None;
        let reverse = repo.posting_values(&request).unwrap();
        assert_eq!(
            reverse
                .values
                .iter()
                .map(|entry| &entry.value)
                .collect::<Vec<_>>(),
            [
                &ScalarKey::String("c".into()),
                &ScalarKey::String("b".into())
            ]
        );
        let selected_query = crate::query::Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(crate::query::parse_filter("tags ?= 'b'").unwrap()),
            options: crate::query::QueryOptions {
                sort: vec![],
                limit: 1,
                offset: 0,
                count: false,
                return_mode: crate::workload::ReturnMode::Locator,
                evidence: false,
            },
        };
        let selection = repo.select_exact("data", &selected_query).unwrap();
        assert_eq!(selection.len(), 1);
        let parameter_query = crate::query::Query {
            filter: Some(crate::query::parse_filter("tags ?= $chosen_tag").unwrap()),
            ..selected_query.clone()
        };
        assert!(repo.select_exact("data", &parameter_query).is_err());
        let parameter_query = parameter_query
            .with_parameters(&std::collections::HashMap::from([(
                "chosen_tag".into(),
                crate::model::Value::String("a".into()),
            )]))
            .unwrap();
        assert_eq!(
            repo.select_exact("data", &parameter_query).unwrap().len(),
            2
        );
        let text_query = crate::query::Query {
            filter: None,
            fts: Some(crate::query::FtsQuery {
                pattern_parameter: None,
                mode: crate::query::FtsMode::Literal,
                case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                literal: "a".into(),
                fields: vec![],
            }),
            ..selected_query.clone()
        };
        let text_selection = repo.select_exact("data", &text_query).unwrap();
        assert_eq!(text_selection.len(), 3);
        let mut text_request = PostingValuesRequest::new("data", "tags", 10);
        text_request.selection = Some(text_selection);
        let text_values = repo.posting_values(&text_request).unwrap();
        assert_eq!(text_values.coverage.eligible_records, 2);
        assert_eq!(text_values.coverage.unavailable_records, 1);
        assert_eq!(
            text_values
                .values
                .iter()
                .map(|entry| entry.count)
                .collect::<Vec<_>>(),
            [2, 1, 1]
        );
        let mut selected_request = PostingValuesRequest::new("data", "tags", 1);
        selected_request.selection = Some(selection.clone());
        let first_selected = repo.posting_values(&selected_request).unwrap();
        assert_eq!(
            first_selected.values,
            [PostingValueCount {
                value: ScalarKey::String("a".into()),
                count: 1
            }]
        );
        assert_eq!(
            first_selected.coverage,
            PostingValuesCoverage {
                eligible_records: 1,
                unavailable_records: 0,
                selected_records: Some(1),
                present_records: 1,
                explicit_null_records: 0
            }
        );
        selected_request.continuation = first_selected.continuation;
        let second_selected = repo.posting_values(&selected_request).unwrap();
        assert_eq!(
            second_selected.values,
            [PostingValueCount {
                value: ScalarKey::String("b".into()),
                count: 1
            }]
        );
        assert!(second_selected.continuation.is_none());
        selected_request.selection = Some(repo.select_exact("data", &selected_query).unwrap());
        assert!(repo.posting_values(&selected_request).is_err());
        let fallback_query = crate::query::Query {
            filter: Some(crate::query::parse_filter("@record.class = 1").unwrap()),
            ..selected_query.clone()
        };
        let fallback_selection = repo.select_exact("data", &fallback_query).unwrap();
        assert_eq!(fallback_selection.len(), 1);
        let mut fallback_count = fallback_query.clone();
        fallback_count.options.count = true;
        fallback_count.options.limit = 0;
        fallback_count.options.offset = 3;
        let fallback_count = repo.search(&fallback_count).unwrap();
        assert_eq!(fallback_count.count, Some(1));
        assert_eq!(fallback_count.counters.files_opened, 0);
        selected_request.selection = Some(fallback_selection);
        selected_request.continuation = None;
        assert_eq!(
            repo.posting_values(&selected_request).unwrap().coverage,
            PostingValuesCoverage {
                eligible_records: 0,
                unavailable_records: 1,
                selected_records: Some(1),
                present_records: 0,
                explicit_null_records: 0
            }
        );
        assert!(
            repo.posting_values(&selected_request)
                .unwrap()
                .values
                .is_empty()
        );
        let held = root.join("held.jsonl");
        std::fs::rename(&path, &held).unwrap();
        assert_eq!(repo.select_exact("data", &selected_query).unwrap().len(), 1);
        assert_eq!(
            repo.posting_values(&request).unwrap().values,
            reverse.values
        );
        assert!(
            repo.posting_values(&selected_request)
                .unwrap()
                .values
                .is_empty()
        );
        std::fs::rename(&held, &path).unwrap();
        repo.save_default_snapshot_for_test();
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        assert_eq!(
            restored.posting_values(&request).unwrap().values,
            reverse.values
        );
        selected_request.selection = Some(selection);
        assert!(restored.posting_values(&selected_request).is_err());
        request.continuation = reverse.continuation;
        assert!(restored.posting_values(&request).is_err());
        request.continuation = None;
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn numeric_and_temporal_keys_keep_their_types_and_order() {
    use crate::model::Number;
    let root = std::env::temp_dir().join(format!("objekte-posting-typed-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("a", r#"{"amount":1,"published":"2026-01-02"}"#),
        ("b", r#"{"amount":1.5,"published":"2026-01-01"}"#),
        (
            "c",
            r#"{"amount":100000000000000000000,"published":"2026-01-02"}"#,
        ),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let mut schema = Schema::default();
    for (name, kind) in [
        ("amount", LogicalType::Number),
        ("published", LogicalType::Date),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let numbers = repo
        .posting_values(&PostingValuesRequest::new("data", "amount", 10))
        .unwrap();
    assert_eq!(
        numbers.values,
        [
            PostingValueCount {
                value: ScalarKey::Number(Number::integer(1)),
                count: 1
            },
            PostingValueCount {
                value: ScalarKey::Number(Number::float(1.5).unwrap()),
                count: 1
            },
            PostingValueCount {
                value: ScalarKey::Number(Number::integer(100000000000000000000)),
                count: 1
            },
        ]
    );
    let dates = repo
        .posting_values(&PostingValuesRequest::new("data", "published", 10))
        .unwrap();
    assert_eq!(dates.values.len(), 2);
    assert!(matches!(dates.values[0].value, ScalarKey::Date(_)));
    assert_eq!((dates.values[0].count, dates.values[1].count), (1, 2));
    let mut narrowed = PostingValuesRequest::new("data", "amount", 10);
    narrowed.lower = Some(PostingValueBound {
        value: ScalarKey::Number(Number::integer(1)),
        inclusive: false,
    });
    narrowed.upper = Some(PostingValueBound {
        value: ScalarKey::Number(Number::integer(2)),
        inclusive: true,
    });
    assert_eq!(
        repo.posting_values(&narrowed).unwrap().values,
        [PostingValueCount {
            value: ScalarKey::Number(Number::float(1.5).unwrap()),
            count: 1
        }]
    );
    narrowed.prefix = Some("1".into());
    assert!(repo.posting_values(&narrowed).is_err());
    std::fs::remove_dir_all(root).unwrap();
}


#[derive(Debug)]
struct FoldPosting {
    complete: bool,
}


impl crate::reference::BindingOperation for FoldPosting {
    fn accepts(&self, kind: &LogicalType, operator: crate::query::Operator, _: bool) -> bool {
        *kind == LogicalType::String && operator == crate::query::Operator::Eq
    }

    fn prepare_literal(
        &self,
        literal: &crate::query::Literal,
        _: &LogicalType,
    ) -> Result<crate::model::Value, String> {
        match literal {
            crate::query::Literal::String(text) => {
                Ok(crate::model::Value::String(text.to_ascii_lowercase()))
            }
            _ => Err("text required".into()),
        }
    }

    fn map_value(&self, value: &crate::model::Value) -> Option<crate::model::Value> {
        match value {
            crate::model::Value::String(text) => {
                Some(crate::model::Value::String(text.to_ascii_lowercase()))
            }
            _ => None,
        }
    }

    fn map_text(&self, text: &str) -> String {
        text.to_ascii_lowercase()
    }

    fn prepare_posting_prefix(&self, prefix: &str) -> Result<Option<String>, String> {
        Ok(Some(prefix.to_ascii_lowercase()))
    }

    fn posting(&self) -> bool {
        true
    }

    fn complete_coverage(&self) -> bool {
        self.complete
    }
}


#[test]
fn named_default_posting_returns_stored_keys_and_rejects_incomplete_coverage() {
    use crate::reference::RegisteredBinding;
    use std::sync::Arc;
    let root = std::env::temp_dir().join(format!("objekte-posting-binding-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, title) in [("a", "HELLO"), ("b", "hello"), ("c", "World")] {
        std::fs::write(
            root.join(format!("data/{name}.json")),
            format!(r#"{{"title":"{title}"}}"#),
        )
        .unwrap();
    }
    for complete in [true, false] {
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["title".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
        schema
            .register_operation(
                vec!["title".into()],
                RegisteredBinding {
                    name: "fold".into(),
                    version: 3,
                    operation: Arc::new(FoldPosting { complete }),
                },
            )
            .unwrap();
        schema
            .set_default_binding(vec!["title".into()], "fold")
            .unwrap();
        let definition = CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        };
        let options = RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        };
        let repo = Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        let request = PostingValuesRequest::new("data", "title", 10);
        if complete {
            assert_eq!(
                repo.posting_values(&request).unwrap().values,
                [
                    PostingValueCount {
                        value: ScalarKey::String("hello".into()),
                        count: 2
                    },
                    PostingValueCount {
                        value: ScalarKey::String("world".into()),
                        count: 1
                    }
                ]
            );
            let mut prefixed = request.clone();
            prefixed.prefix = Some("HE".into());
            assert_eq!(
                repo.posting_values(&prefixed).unwrap().values,
                [PostingValueCount {
                    value: ScalarKey::String("hello".into()),
                    count: 2
                }]
            );
            let mut paged = PostingValuesRequest::new("data", "title", 1);
            let first = repo.posting_values(&paged).unwrap();
            paged.continuation = first.continuation.clone();
            let mut changed_binding = paged.clone();
            changed_binding.binding = Some("default".into());
            assert!(
                repo.posting_values(&changed_binding)
                    .unwrap_err()
                    .contains("continuation scope")
            );
            let snapshot = root.join("fold.snapshot");
            repo.save_snapshot(&snapshot).unwrap();
            let restored = crate::storage::load_snapshot(
                root.clone(),
                vec![definition.clone()],
                options,
                &crate::source::SourceRegistry::builtin(),
                &snapshot,
            )
            .unwrap();
            assert_eq!(
                restored.posting_values(&request).unwrap().values,
                repo.posting_values(&request).unwrap().values
            );
            assert!(
                restored
                    .posting_values(&paged)
                    .unwrap_err()
                    .contains("continuation scope")
            );
        } else {
            assert!(
                repo.posting_values(&request)
                    .unwrap_err()
                    .contains("incomplete")
            );
        }
        let mut explicit = request;
        explicit.binding = Some("default".into());
        assert_eq!(repo.posting_values(&explicit).unwrap().values.len(), 3);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_prefix_ranges_and_continuations_visit_only_relevant_keys() {
    let root = std::env::temp_dir().join(format!("objekte-posting-bounded-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut lines = String::new();
    for number in 0..1000 {
        lines.push_str(&format!(r#"{{"key":"k{number:04}"}}"#));
        lines.push('\n');
    }
    for value in ["\u{10ffff}", "\u{10ffff}x"] {
        lines.push_str(&serde_json::json!({"key":value}).to_string());
        lines.push('\n');
    }
    std::fs::write(root.join("data/values.jsonl"), lines).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["key".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    let mut request = PostingValuesRequest::new("data", "key", 3);
    request.prefix = Some("k099".into());
    let first = repo.posting_values(&request).unwrap();
    assert_eq!(
        first
            .values
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        ["k0990", "k0991", "k0992"].map(|key| ScalarKey::String(key.into()))
    );
    assert_eq!(first.visited_keys, 4);
    let cursor = first.continuation.clone().unwrap();
    let mut changed = request.clone();
    changed.prefix = Some("k098".into());
    changed.continuation = Some(cursor.clone());
    assert!(repo.posting_values(&changed).is_err());
    changed.prefix = request.prefix.clone();
    changed.limit = 0;
    changed.descending = true;
    assert!(repo.posting_values(&changed).is_err());
    changed.descending = false;
    changed.lower = Some(PostingValueBound {
        value: ScalarKey::String("k0991".into()),
        inclusive: true,
    });
    assert!(repo.posting_values(&changed).is_err());
    request.continuation = Some(cursor);
    let mut collected = first
        .values
        .into_iter()
        .map(|entry| entry.value)
        .collect::<Vec<_>>();
    loop {
        let page = repo.posting_values(&request).unwrap();
        assert!(page.visited_keys <= 4);
        collected.extend(page.values.into_iter().map(|entry| entry.value));
        match page.continuation {
            Some(next) => request.continuation = Some(next),
            None => break,
        }
    }
    assert_eq!(
        collected,
        (990..1000)
            .map(|number| ScalarKey::String(format!("k{number:04}")))
            .collect::<Vec<_>>()
    );
    request.continuation = None;
    request.prefix = None;
    request.lower = Some(PostingValueBound {
        value: ScalarKey::String("k0500".into()),
        inclusive: true,
    });
    request.upper = Some(PostingValueBound {
        value: ScalarKey::String("k0503".into()),
        inclusive: false,
    });
    let range = repo.posting_values(&request).unwrap();
    assert_eq!(
        range
            .values
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        (500..503)
            .map(|number| ScalarKey::String(format!("k{number:04}")))
            .collect::<Vec<_>>()
    );
    assert_eq!(range.visited_keys, 3);
    request.descending = true;
    let descending = repo.posting_values(&request).unwrap();
    assert_eq!(
        descending
            .values
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        (500..503)
            .rev()
            .map(|number| ScalarKey::String(format!("k{number:04}")))
            .collect::<Vec<_>>()
    );
    request.lower = Some(PostingValueBound {
        value: ScalarKey::String("k0501".into()),
        inclusive: true,
    });
    request.upper = Some(PostingValueBound {
        value: ScalarKey::String("k0501".into()),
        inclusive: true,
    });
    assert_eq!(repo.posting_values(&request).unwrap().values.len(), 1);
    request.upper.as_mut().unwrap().inclusive = false;
    assert!(repo.posting_values(&request).unwrap().values.is_empty());
    request.lower = None;
    request.upper = None;
    request.descending = false;
    request.prefix = Some("\u{10ffff}".into());
    let unicode = repo.posting_values(&request).unwrap();
    assert_eq!(
        unicode
            .values
            .iter()
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        [
            ScalarKey::String("\u{10ffff}".into()),
            ScalarKey::String("\u{10ffff}x".into())
        ]
    );
    request.prefix = Some(String::new());
    assert_eq!(
        repo.posting_values(&request).unwrap().values[0].value,
        ScalarKey::String("k0000".into())
    );
    request.lower = Some(PostingValueBound {
        value: ScalarKey::Number(crate::model::Number::integer(1)),
        inclusive: true,
    });
    assert!(repo.posting_values(&request).is_err());
    request.lower = None;
    request.limit = MAX_POSTING_VALUES_PAGE + 1;
    assert!(repo.posting_values(&request).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
