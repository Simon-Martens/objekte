use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::LogicalType;
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::FieldDefinition;
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;


#[test]
fn validates_binding_without_reopening_source_records() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("objekte-query-validation-{unique}"));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let source = root.join("data/record.json");
    std::fs::write(&source, r#"{"year":1900}"#).unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["year".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = Repository::open_with_options(&root, vec![definition], RepositoryOptions::default())
        .unwrap();
    std::fs::remove_file(&source).unwrap();
    let query = |filter: &str| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 1,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert!(repo.validate_query(&query("year >= 1900")).is_ok());
    assert!(
        repo.validate_query(&query("year >= 'nineteen hundred'"))
            .is_err()
    );
    let mut wrong_scope = query("year >= 1900");
    wrong_scope.collections = vec!["missing".into()];
    assert!(
        repo.validate_query(&wrong_scope)
            .unwrap_err()
            .contains("scope")
    );
    std::fs::remove_dir_all(root).unwrap();
}
