//! Exact posting restrictions plus one residual scan over surviving records.

use std::collections::BTreeMap;

use roaring::RoaringBitmap;
use unicode_casefold::UnicodeCaseFold;

use crate::model::{PathRoot, Value};
use crate::query::{FtsQuery, Operator};
use crate::reference::{BoundExpr, BoundTerm, Derived, RecordContext, Transform, eval_term};
use crate::repository::{CollectionState, Repository};

pub(crate) struct Proof {
    pub term: BoundTerm,
    pub matches: RoaringBitmap,
}
pub(crate) struct Plan {
    pub candidates: Option<RoaringBitmap>,
    pub exact: bool,
    pub proofs: Vec<Proof>,
}

fn term_set(
    repo: &Repository,
    collection: &CollectionState,
    term: &BoundTerm,
    domain: &RoaringBitmap,
) -> Result<Option<RoaringBitmap>, String> {
    if let PathRoot::Metadata(_) = term.path.root {
        let empty = Value::Object(BTreeMap::new());
        let mut result = RoaringBitmap::new();
        for record in &collection.generation.records {
            let file = &repo.files.files[record.file.0 as usize];
            let ctx = RecordContext {
                value: &empty,
                record,
                file,
                collection_name: &collection.definition.name,
                repository_root: &repo.files.root,
            };
            if eval_term(term, &ctx)? {
                result.insert(record.slot.0);
            }
        }
        return Ok(Some(result));
    }
    Ok(collection
        .postings
        .fields
        .get(&term.path.segments)
        .and_then(|index| index.exact_set(term, domain)))
}

pub(crate) fn plan_expr(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
) -> Result<Plan, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let exact_candidates = term_set(repo, collection, term, domain)?;
            let proofs = exact_candidates
                .as_ref()
                .map(|set| {
                    vec![Proof {
                        term: term.clone(),
                        matches: set.clone(),
                    }]
                })
                .unwrap_or_default();
            let exact = exact_candidates.is_some();
            let candidates = if exact {
                exact_candidates
            } else {
                custom_candidates(collection, term)
            };
            Plan {
                candidates,
                exact,
                proofs,
            }
        }
        BoundExpr::And(parts) => {
            let mut children = parts
                .iter()
                .map(|p| plan_expr(repo, collection, p, domain))
                .collect::<Result<Vec<_>, _>>()?;
            let exact = children.iter().all(|p| p.exact);
            children.sort_by_key(|p| p.candidates.as_ref().map(|s| s.len()).unwrap_or(u64::MAX));
            let mut candidates = None;
            let mut proofs = Vec::new();
            for child in children {
                if let Some(set) = child.candidates {
                    candidates = Some(match candidates {
                        None => set,
                        Some(prior) => &prior & &set,
                    });
                }
                proofs.extend(child.proofs);
                if candidates.as_ref().is_some_and(RoaringBitmap::is_empty) {
                    break;
                }
            }
            Plan {
                candidates,
                exact,
                proofs,
            }
        }
        BoundExpr::Or(parts) => {
            let children = parts
                .iter()
                .map(|p| plan_expr(repo, collection, p, domain))
                .collect::<Result<Vec<_>, _>>()?;
            if children.iter().all(|p| p.candidates.is_some()) {
                let mut candidates = RoaringBitmap::new();
                let mut proofs = Vec::new();
                let exact = children.iter().all(|p| p.exact);
                for child in children {
                    candidates |= child.candidates.unwrap();
                    proofs.extend(child.proofs);
                }
                Plan {
                    candidates: Some(candidates),
                    exact,
                    proofs,
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proofs: Vec::new(),
                }
            }
        }
        BoundExpr::Not(inner) => {
            let child = plan_expr(repo, collection, inner, domain)?;
            if child.exact {
                Plan {
                    candidates: Some(domain - &child.candidates.unwrap()),
                    exact: true,
                    proofs: Vec::new(),
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proofs: Vec::new(),
                }
            }
        }
    })
}

fn custom_candidates(collection: &CollectionState, term: &BoundTerm) -> Option<RoaringBitmap> {
    if term.derived != Derived::None || !matches!(term.operator, Operator::Contains | Operator::Eq)
    {
        return None;
    }
    if matches!(term.binding_name.as_str(), "default" | "lower") {
        return None;
    }
    let Value::String(literal) = &term.literal else {
        return None;
    };
    if literal.contains(['%', '\\']) {
        return None;
    }
    collection
        .custom_text
        .get(&(term.path.segments.clone(), term.binding_name.clone()))?
        .candidates(literal)
}

pub(crate) fn fts_candidates(
    collection: &CollectionState,
    fts: &FtsQuery,
) -> Option<RoaringBitmap> {
    if fts.fields.is_empty() {
        return collection.raw.candidates(&fts.literal);
    }
    let mut union = RoaringBitmap::new();
    for field in &fts.fields {
        let candidates = if let Some(name) = &field.binding {
            let key = (field.path.segments.clone(), name.clone());
            let binding = collection.definition.schema.custom.get(&key)?;
            let literal = if binding.transform == Transform::CaseFold {
                fts.literal.case_fold().collect::<String>()
            } else {
                fts.literal.clone()
            };
            collection.custom_text.get(&key)?.candidates(&literal)?
        } else {
            collection.raw.candidates(&fts.literal)?
        };
        union |= candidates;
    }
    Some(union)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LogicalType;
    use crate::query::{FtsField, FtsQuery, Query, QueryOptions, parse_filter};
    use crate::reference::{CustomBinding, FieldDefinition, Transform};
    use crate::repository::{CollectionDefinition, IdentityPolicy};
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

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
}
