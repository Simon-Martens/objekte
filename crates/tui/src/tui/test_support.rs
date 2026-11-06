//! Small on-disk repository shared by terminal interaction tests.

use objekte_search_core::model::{FieldPath, LogicalType};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
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
            snapshot_mode: SnapshotMode::Disabled,
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
        })
        .unwrap();
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        schema,
        sort_fields: vec![FieldPath::parse("number").unwrap()],
        sort_locale: "en".into(),
    }
}
