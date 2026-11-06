use super::*;
use crate::model::LogicalType;
use crate::query::{FtsField, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{CustomBinding, FieldDefinition, Transform};
use crate::repository::{CollectionDefinition, IdentityPolicy};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;
use std::sync::Arc;

#[derive(Debug)]
struct RemoveSpaces;

impl crate::reference::BindingOperation for RemoveSpaces {
    fn accepts(&self, kind: &LogicalType, op: Operator, _: bool) -> bool {
        *kind == LogicalType::String && matches!(op, Operator::Contains | Operator::Eq)
    }

    fn prepare_literal(
        &self,
        literal: &crate::query::Literal,
        _: &LogicalType,
    ) -> Result<Value, String> {
        match literal {
            crate::query::Literal::String(text) => Ok(Value::String(self.map_text(text))),
            _ => Err("expected text".into()),
        }
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        match value {
            Value::String(text) => Some(Value::String(self.map_text(text))),
            _ => None,
        }
    }

    fn map_text(&self, text: &str) -> String {
        text.chars().filter(|ch| *ch != ' ').collect()
    }

    fn bigram(&self) -> bool {
        true
    }
}

#[test]
fn transformed_raw_index_cannot_prune_identity_field_predicates() {
    use crate::reference::{RegisteredBinding, Schema};
    use crate::repository::{RepositoryOptions, SnapshotMode};
    let root = std::env::temp_dir().join(format!("objekte-raw-compat-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), r#"{"title":"hello world"}"#).unwrap();
    std::fs::write(root.join("data/b.json"), r#"{"title":"other"}"#).unwrap();
    let definition = || {
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
            .set_raw_binding(RegisteredBinding {
                name: "nospace".into(),
                version: 1,
                operation: Arc::new(RemoveSpaces),
            })
            .unwrap();
        CollectionDefinition {
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
        }
    };
    for alphabet in [
        crate::workload::BigramAlphabet::Ascii,
        crate::workload::BigramAlphabet::Unicode,
    ] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::ReadWrite,
            ..Default::default()
        };
        for open in 0..2 {
            let repo = Repository::open_with_options(&root, vec![definition()], options).unwrap();
            assert_eq!(repo.startup().restored, open == 1);
            for filter in [
                Some("title ~ 'hello world'"),
                Some("title = 'hello world'"),
                None,
            ] {
                let query = Query {
                    collections: vec!["data".into()],
                    filter: filter.map(|s| parse_filter(s).unwrap()),
                    fts: filter.is_none().then(|| FtsQuery {
                        literal: "hello world".into(),
                        fields: vec![FtsField {
                            path: crate::model::FieldPath::parse("title").unwrap(),
                            binding: None,
                        }],
                    }),
                    options: QueryOptions {
                        sort: vec![],
                        limit: 10,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: true,
                    },
                };
                let result = repo.search(&query).unwrap();
                assert_eq!(result.count, Some(1), "{alphabet:?} {filter:?}");
                assert_eq!(result.hits[0].record_id, "a");
                if filter == Some("title ~ 'hello world'") {
                    let plan = repo.explain(&query).unwrap();
                    assert_ne!(plan[0].terms[0].selected.kind, AccessPathKind::RawBigram);
                    assert!(plan[0].terms[0].reason.contains("raw bigram rejected"));
                }
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[derive(Debug)]
struct KeyedOperation {
    posting: bool,
    bigram: bool,
    complete: bool,
}

impl crate::reference::BindingOperation for KeyedOperation {
    fn accepts(&self, kind: &LogicalType, operator: Operator, _multi: bool) -> bool {
        *kind == LogicalType::String
            && matches!(operator, Operator::Eq | Operator::Ne | Operator::Contains)
    }

    fn prepare_literal(
        &self,
        literal: &crate::query::Literal,
        _kind: &LogicalType,
    ) -> Result<Value, String> {
        match literal {
            crate::query::Literal::Null => Ok(Value::Null),
            crate::query::Literal::String(text) => text
                .strip_prefix("key:")
                .map(|body| Value::String(body.trim().to_lowercase()))
                .ok_or("keyed literal needs key: prefix".into()),
            _ => Err("keyed literal requires text".into()),
        }
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        match value {
            Value::String(text) => Some(Value::String(self.map_text(text))),
            Value::Null => Some(Value::Null),
            _ => None,
        }
    }

    fn map_text(&self, text: &str) -> String {
        text.trim().to_lowercase()
    }

    fn posting(&self) -> bool {
        self.posting
    }

    fn bigram(&self) -> bool {
        self.bigram
    }

    fn complete_coverage(&self) -> bool {
        self.complete
    }
}

#[test]
fn registered_binding_has_identical_filter_fts_and_restore_semantics() {
    use crate::query::{FtsField, Literal};
    use crate::reference::RegisteredBinding;
    use crate::repository::{RepositoryOptions, SnapshotMode};

    let root = std::env::temp_dir().join(format!("objekte-keyed-binding-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for (id, json) in [
        ("a", r#"{"title":"  ALPHA  ","text":"NEEDLE Place"}"#),
        ("b", r#"{"title":"alpha","text":"other"}"#),
        ("c", r#"{"title":"beta","text":"needle!"}"#),
        ("d", r#"{"text":"other"}"#),
        ("e", r#"{"title":"","text":"other"}"#),
    ] {
        std::fs::write(path.join(format!("{id}.json")), json).unwrap();
    }
    let definition = |posting, bigram, complete, version| {
        let mut schema = crate::reference::Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["title".into()],
                kind: LogicalType::String,
                required: false,
                nullable: true,
                indexed: false,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
        schema
            .register_field(FieldDefinition {
                path: vec!["text".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
        let binding = RegisteredBinding {
            name: "keyed".into(),
            version,
            operation: Arc::new(KeyedOperation {
                posting,
                bigram,
                complete,
            }),
        };
        schema
            .register_operation(vec!["title".into()], binding.clone())
            .unwrap();
        schema
            .set_default_binding(vec!["title".into()], "keyed")
            .unwrap();
        schema.set_raw_binding(binding).unwrap();
        CollectionDefinition {
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
        }
    };
    let make_query = |filter: Option<&str>, fts: Option<(&str, Option<&str>)>| Query {
        collections: vec![],
        fts: fts.map(|(literal, selected)| FtsQuery {
            literal: literal.into(),
            fields: selected
                .map(|name| {
                    vec![FtsField {
                        path: crate::model::FieldPath::parse("title").unwrap(),
                        binding: Some(name.into()),
                    }]
                })
                .unwrap_or_default(),
        }),
        filter: filter.map(|text| parse_filter(text).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let mut default_fts = make_query(None, Some(("alp", Some("keyed"))));
    default_fts.fts.as_mut().unwrap().fields[0].binding = None;
    let cases = [
        (
            make_query(Some("title:keyed = 'key:ALPHA'"), None),
            vec!["a", "b"],
            vec!["title"],
        ),
        (
            make_query(Some("title = 'key:ALPHA'"), None),
            vec!["a", "b"],
            vec!["title"],
        ),
        (
            make_query(Some("title:keyed != 'key:ALPHA'"), None),
            vec!["c", "e"],
            vec![],
        ),
        (
            make_query(Some("title:keyed ~ 'key:alp'"), None),
            vec!["a", "b"],
            vec!["title"],
        ),
        (
            make_query(Some("title:keyed ~ 'key:al'"), None),
            vec!["a", "b"],
            vec!["title"],
        ),
        (
            make_query(Some("title:keyed ~ 'key:'"), None),
            vec!["a", "b", "c", "e"],
            vec!["title"],
        ),
        (
            make_query(None, Some(("alp", Some("keyed")))),
            vec!["a", "b"],
            vec!["title"],
        ),
        (default_fts, vec!["a", "b"], vec!["title"]),
        (
            make_query(None, Some(("", Some("keyed")))),
            vec!["a", "b", "c", "e"],
            vec!["title"],
        ),
        (
            make_query(None, Some(("needle", None))),
            vec!["a", "c"],
            vec!["text"],
        ),
    ];
    for (posting, bigram, complete) in [
        (true, true, true),
        (false, true, true),
        (false, false, true),
        (true, true, false),
    ] {
        let repo = Repository::open_with_options(
            &root,
            vec![definition(posting, bigram, complete, 1)],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        for (case_index, (query, expected, evidence)) in cases.iter().enumerate() {
            let result = repo.search(query).unwrap();
            let reference = repo.search_reference(query).unwrap();
            let mut ids = result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>();
            ids.sort_unstable();
            assert_eq!(
                &ids, expected,
                "posting={posting}, bigram={bigram}, complete={complete}"
            );
            assert_eq!(result.count.unwrap(), reference.count.unwrap());
            assert_eq!(result.hits, reference.hits);
            assert!(
                result
                    .hits
                    .iter()
                    .all(|hit| hit.matched_fields == *evidence)
            );
            if case_index == 0 && posting && complete {
                assert_eq!(result.counters.files_opened, 0);
            }
            if case_index == 6 && bigram && complete {
                assert!(result.counters.candidates < 5);
            }
        }
        let invalid = make_query(Some("title:keyed > 'key:alpha'"), None);
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unsupported operation")
        );
        let invalid = make_query(Some("title:keyed = 'alpha'"), None);
        assert!(repo.search(&invalid).unwrap_err().contains("key: prefix"));
        let invalid = make_query(Some("title:unknown = 'key:alpha'"), None);
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unknown binding")
        );
        let invalid = make_query(None, Some(("alpha", Some("unknown"))));
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unknown FTS binding")
        );
        let _ = Literal::Null;
    }
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let built =
        Repository::open_with_options(&root, vec![definition(true, true, true, 1)], options)
            .unwrap();
    assert!(!built.startup().restored);
    drop(built);
    let restored =
        Repository::open_with_options(&root, vec![definition(true, true, true, 1)], options)
            .unwrap();
    assert!(restored.startup().restored);
    for (query, expected, _) in &cases {
        let mut ids = restored
            .search(query)
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(
            ids.iter().map(String::as_str).collect::<Vec<_>>(),
            *expected
        );
    }
    drop(restored);
    let changed =
        Repository::open_with_options(&root, vec![definition(true, true, true, 2)], options)
            .unwrap();
    assert!(!changed.startup().restored);
    std::fs::remove_dir_all(root).unwrap();
}

fn plan_expr(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
) -> Result<Plan, String> {
    plan_expr_restricted(repo, collection, expr, domain, domain, true)
}

#[test]
fn record_id_lookup_respects_case_fold_binding() {
    let root = std::env::temp_dir().join(format!("objekte-id-fold-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for id in ["ABC", "abc", "Straße", "STRASSE"] {
        std::fs::write(path.join(format!("{id}.json")), "{}").unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema: Default::default(),
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }],
        crate::repository::RepositoryOptions {
            snapshot_mode: crate::repository::SnapshotMode::Disabled,
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, expected) in [
        ("@record.id = 'abc'", vec!["abc"]),
        ("@record.id = 'ABC'", vec!["ABC"]),
        ("@record.id:lower = 'abc'", vec!["ABC", "abc"]),
        ("@record.id:lower = 'STRASSE'", vec!["STRASSE", "Straße"]),
    ] {
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let mut actual = optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>();
        actual.sort_unstable();
        assert_eq!(actual, expected, "{filter}");
        assert_eq!(
            optimized.count.unwrap(),
            reference.count.unwrap(),
            "{filter}"
        );
        assert_eq!(optimized.hits, reference.hits, "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn indexed_restriction_precedes_written_first_scan() {
    let root = std::env::temp_dir().join(format!("objekte-executor-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for (id, kind, text) in [
        ("a", "wanted", "needle"),
        ("b", "other", "needle"),
        ("c", "other", "nope"),
    ] {
        std::fs::write(
            path.join(format!("{id}.json")),
            serde_json::json!({"kind":kind,"text":text}).to_string(),
        )
        .unwrap();
    }
    let mut schema = crate::reference::Schema::default();
    for (name, indexed) in [("kind", true), ("text", false)] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    schema
        .register_custom(
            vec!["text".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    let repo = Repository::open(
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
    )
    .unwrap();
    let domain: RoaringBitmap = (0..3).collect();
    let bound = crate::reference::bind_filter(
        &parse_filter("kind = 'wanted'").unwrap(),
        &repo.collections[0].definition.schema,
    )
    .unwrap();
    let plan = plan_expr(&repo, &repo.collections[0], &bound, &domain).unwrap();
    assert_eq!(plan.choices[0].selected.kind, AccessPathKind::Posting);
    assert!(
        plan.choices[0]
            .alternatives
            .iter()
            .any(|p| p.kind == AccessPathKind::Scanner)
    );
    let bound = crate::reference::bind_filter(
        &parse_filter("kind ~ 'want'").unwrap(),
        &repo.collections[0].definition.schema,
    )
    .unwrap();
    let plan = plan_expr(&repo, &repo.collections[0], &bound, &domain).unwrap();
    assert_eq!(plan.choices[0].selected.kind, AccessPathKind::Dictionary);
    assert!(plan.exact);
    let options = QueryOptions {
        sort: vec![],
        limit: 50,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: true,
    };
    for expression in [
        "text ~ 'needle' && kind = 'wanted'",
        "kind = 'wanted' && text ~ 'needle'",
    ] {
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(expression).unwrap()),
            options: options.clone(),
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count.unwrap(), reference.count.unwrap());
        assert_eq!(optimized.hits[0].record_id, "a");
        assert_eq!(optimized.counters.record_parses, 1);
    }
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("text ~ 'needle' && kind = 'absent'").unwrap()),
        options,
    };
    let optimized = repo.search(&query).unwrap();
    assert_eq!(optimized.count.unwrap(), 0);
    assert_eq!(optimized.counters.files_opened, 0);
    let identity = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("@record.id = 'b'").unwrap()),
        options: query.options.clone(),
    };
    let optimized = repo.search(&identity).unwrap();
    assert_eq!(optimized.count.unwrap(), 1);
    assert_eq!(optimized.hits[0].record_id, "b");
    assert_eq!(optimized.counters.files_opened, 0);
    let disjunction = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("kind = 'wanted' || text ~ 'nope'").unwrap()),
        options: query.options.clone(),
    };
    let optimized = repo.search(&disjunction).unwrap();
    assert_eq!(optimized.count.unwrap(), 2);
    assert_eq!(
        optimized.count.unwrap(),
        repo.search_reference(&disjunction).unwrap().count.unwrap()
    );
    let folded = Query {
        collections: vec![],
        fts: Some(FtsQuery {
            literal: "NEEDLE".into(),
            fields: vec![FtsField {
                path: crate::model::FieldPath::parse("text").unwrap(),
                binding: Some("folded".into()),
            }],
        }),
        filter: None,
        options: query.options.clone(),
    };
    let optimized = repo.search(&folded).unwrap();
    assert_eq!(optimized.count.unwrap(), 2);
    assert_eq!(optimized.counters.record_parses, 2);
    assert_eq!(
        optimized.count.unwrap(),
        repo.search_reference(&folded).unwrap().count.unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rare_text_restriction_can_precede_a_broad_posting() {
    let root = std::env::temp_dir().join(format!("objekte-adapt-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for i in 0..240 {
        let text = if i == 173 {
            "uniqueneedleword"
        } else {
            "ordinary text"
        };
        std::fs::write(
            path.join(format!("{i:03}.json")),
            serde_json::json!({"kind":"broad","text":text}).to_string(),
        )
        .unwrap();
    }
    let mut schema = crate::reference::Schema::default();
    for (name, indexed) in [("kind", true), ("text", false)] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let repo = Repository::open(
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
    )
    .unwrap();
    let domain: RoaringBitmap = (0..240).collect();
    let options = QueryOptions {
        sort: vec![],
        limit: 10,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence: false,
    };
    for expression in [
        "kind = 'broad' && text ~ 'uniqueneedleword'",
        "text ~ 'uniqueneedleword' && kind = 'broad'",
    ] {
        let bound = crate::reference::bind_filter(
            &parse_filter(expression).unwrap(),
            &repo.collections[0].definition.schema,
        )
        .unwrap();
        let plan = plan_expr(&repo, &repo.collections[0], &bound, &domain).unwrap();
        assert_eq!(plan.choices[0].field, "text");
        assert_eq!(plan.choices[0].selected.kind, AccessPathKind::RawBigram);
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(expression).unwrap()),
            options: options.clone(),
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count.unwrap(), 1);
        assert_eq!(optimized.hits[0].record_id, "173");
        assert_eq!(optimized.hits[0].record_id, reference.hits[0].record_id);
        assert_eq!(optimized.counters.record_parses, 1);
    }
    for expression in [
        "kind = 'absent' || text ~ 'uniqueneedleword'",
        "text ~ 'uniqueneedleword' || kind = 'absent'",
    ] {
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter(expression).unwrap()),
            options: options.clone(),
        };
        let optimized = repo.search(&query).unwrap();
        assert_eq!(
            optimized.count.unwrap(),
            repo.search_reference(&query).unwrap().count.unwrap()
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
