//! Small on-disk repository shared by terminal interaction tests.

use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions,
};
use objekte_search_core::source::SourceFormat;

use super::app::App;


/// Fixture owner keeps source files alive while asynchronous queries run.
pub(super) struct Fixture {
    pub(super) _temp: tempfile::TempDir,
    pub(super) app: App,
}


pub(super) fn fixture(count: usize) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    for collection in ["docs", "notes"] {
        let directory = temp.path().join(collection);
        std::fs::create_dir_all(&directory).unwrap();
        let records = if collection == "docs" { count } else { 1 };
        for index in 0..records {
            let title = if index == 0 {
                "needle title"
            } else {
                "ordinary title"
            };
            std::fs::write(
                directory.join(format!("{index:04}.json")),
                format!(r#"{{"title":"{title}","number":{index}}}"#),
            )
            .unwrap();
        }
    }
    let definitions = ["docs", "notes"].into_iter().map(definition).collect();
    let repo = Repository::open_with_options(
        temp.path(),
        definitions,
        RepositoryOptions {
            ..RepositoryOptions::default()
        },
    )
    .unwrap();
    Fixture {
        app: App::new(repo),
        _temp: temp,
    }
}

pub(super) fn relation_fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    for collection in ["entries", "mentions"] {
        std::fs::create_dir_all(temp.path().join(collection)).unwrap();
    }
    for (path, body) in [
        (
            "entries/a.json",
            r#"{"id":"A","mention":"m1","title":"First"}"#,
        ),
        (
            "entries/b.json",
            r#"{"id":"B","mention":"lost","title":"Dangling"}"#,
        ),
        ("entries/c.json", r#"{"id":"C","title":"Missing"}"#),
        ("mentions/m1.json", r#"{"id":"m1","title":"Target"}"#),
    ] {
        std::fs::write(temp.path().join(path), body).unwrap();
    }
    let mut entries = Schema::default();
    let mut mentions = Schema::default();
    for schema in [&mut entries, &mut mentions] {
        for name in ["id", "title"] {
            schema
                .register_field(FieldDefinition {
                    path: vec![name.into()],
                    kind: if name == "id" {
                        LogicalType::Id
                    } else {
                        LogicalType::String
                    },
                    required: true,
                    nullable: false,
                    indexed: true,
                    multi: false,
                    element_nullable: Vec::new(),
                })
                .unwrap();
        }
    }
    entries
        .register_field(FieldDefinition {
            path: vec!["mention".into()],
            kind: LogicalType::Id,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    entries
        .register_relation(RelationDefinition {
            path: vec!["mention".into()],
            target_collection: "mentions".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("entry_mentions".into()),
        })
        .unwrap();
    let definition = |name: &str, schema| CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: Vec::new(),
        sort_locale: "en".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let repo = Repository::open_with_options(
        temp.path(),
        vec![
            definition("entries", entries),
            definition("mentions", mentions),
        ],
        RepositoryOptions {
            ..RepositoryOptions::default()
        },
    )
    .unwrap();
    Fixture {
        app: App::new(repo),
        _temp: temp,
    }
}

fn definition(name: &str) -> CollectionDefinition {
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["number".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("number").unwrap()],
        sort_locale: "en".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}
