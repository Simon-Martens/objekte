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
            snapshot_mode: SnapshotMode::ReadWrite,
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
        let held = root.join("held.jsonl");
        std::fs::rename(&path, &held).unwrap();
        assert_eq!(
            repo.posting_values(&request).unwrap().values,
            reverse.values
        );
        std::fs::rename(&held, &path).unwrap();
        let restored =
            Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
        assert!(restored.startup().restored);
        assert_eq!(
            restored.posting_values(&request).unwrap().values,
            reverse.values
        );
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
                Ok(crate::model::Value::String(text.to_lowercase()))
            }
            _ => Err("text required".into()),
        }
    }

    fn map_value(&self, value: &crate::model::Value) -> Option<crate::model::Value> {
        match value {
            crate::model::Value::String(text) => {
                Some(crate::model::Value::String(text.to_lowercase()))
            }
            _ => None,
        }
    }

    fn map_text(&self, text: &str) -> String {
        text.to_lowercase()
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
                indexed: false,
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
        } else {
            assert!(
                repo.posting_values(&request)
                    .unwrap_err()
                    .contains("incomplete")
            );
        }
        let mut explicit = request;
        explicit.binding = Some("default".into());
        assert!(repo.posting_values(&explicit).is_err());
    }
    std::fs::remove_dir_all(root).unwrap();
}
