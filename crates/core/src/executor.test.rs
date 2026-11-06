use super::*;
use crate::model::LogicalType;
use crate::query::{FtsField, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{CustomBinding, FieldDefinition, Transform};
use crate::repository::{CollectionDefinition, IdentityPolicy};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;

fn plan_expr(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
) -> Result<Plan, String> {
    plan_expr_restricted(repo, collection, expr, domain, domain)
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
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
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
        assert_eq!(optimized.count, reference.count);
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
    assert_eq!(optimized.count, 0);
    assert_eq!(optimized.counters.files_opened, 0);
    let identity = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("@record.id = 'b'").unwrap()),
        options: query.options.clone(),
    };
    let optimized = repo.search(&identity).unwrap();
    assert_eq!(optimized.count, 1);
    assert_eq!(optimized.hits[0].record_id, "b");
    assert_eq!(optimized.counters.files_opened, 0);
    let disjunction = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("kind = 'wanted' || text ~ 'nope'").unwrap()),
        options: query.options.clone(),
    };
    let optimized = repo.search(&disjunction).unwrap();
    assert_eq!(optimized.count, 2);
    assert_eq!(
        optimized.count,
        repo.search_reference(&disjunction).unwrap().count
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
    assert_eq!(optimized.count, 2);
    assert_eq!(optimized.counters.record_parses, 2);
    assert_eq!(
        optimized.count,
        repo.search_reference(&folded).unwrap().count
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
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
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
        assert_eq!(optimized.count, 1);
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
            optimized.count,
            repo.search_reference(&query).unwrap().count
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
