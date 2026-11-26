//! Fresh, validated typed query bindings for each request.

use std::collections::HashMap;
use std::sync::Arc;

use crate::query::Query;
use crate::query::binding::filter::BoundExpr;
use crate::query::binding::fts::BoundFts;
use crate::query_predicate::{PredicateBuilder, PreparedExpr, PreparedPredicates, TermId};
use crate::repository::Repository;

#[cfg(test)]
thread_local! {
    static PRODUCTION_PREPARE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn production_prepare_calls() -> usize {
    PRODUCTION_PREPARE_CALLS.with(std::cell::Cell::get)
}

/// One named, fully bound collection entry. The first `prepare` call installs
/// every syntactic leaf and its request-local runtime identifier.
#[derive(Clone)]
pub(crate) struct PreparedCollectionQuery {
    pub(crate) collection_index: usize,
    pub(crate) filter: Option<BoundExpr>,
    pub(crate) fts: Option<BoundFts>,
    pub(crate) filter_shape: Option<PreparedExpr>,
    pub(crate) text_term: Option<TermId>,
}

impl PreparedCollectionQuery {
    pub(crate) fn filter(&self) -> Option<&BoundExpr> {
        self.filter.as_ref()
    }

    pub(crate) fn fts(&self) -> Option<&BoundFts> {
        self.fts.as_ref()
    }

    pub(crate) fn term_id_for_bound_term(
        &self,
        target: &crate::query::binding::filter::BoundTerm,
    ) -> Option<TermId> {
        fn align(
            expr: &crate::query::binding::filter::BoundExpr,
            shape: &PreparedExpr,
            target: &crate::query::binding::filter::BoundTerm,
        ) -> Option<TermId> {
            match (expr, shape) {
                (crate::query::binding::filter::BoundExpr::Term(term), PreparedExpr::Term(id))
                    if std::ptr::eq(term, target) =>
                {
                    Some(*id)
                }
                (
                    crate::query::binding::filter::BoundExpr::And(parts)
                    | crate::query::binding::filter::BoundExpr::Or(parts),
                    PreparedExpr::And(shapes) | PreparedExpr::Or(shapes),
                ) => parts
                    .iter()
                    .zip(shapes)
                    .find_map(|(expr, shape)| align(expr, shape, target)),
                (
                    crate::query::binding::filter::BoundExpr::Not(inner),
                    PreparedExpr::Not(shape),
                ) => align(inner, shape, target),
                _ => None,
            }
        }
        align(self.filter.as_ref()?, self.filter_shape.as_ref()?, target)
    }
}

/// Bound query terms for the selected collection generations.
#[derive(Clone)]
pub(crate) struct PreparedQuery {
    pub(crate) collections: Vec<PreparedCollectionQuery>,
    pub(crate) predicates: Arc<PreparedPredicates>,
}

/// Include the collections reached by the same path resolver used by filter,
/// text and sort binding. Keep the directory positions stable for cached rows.
pub(crate) fn segment_scope(repo: &Repository, query: &Query) -> Vec<bool> {
    use crate::query::binding::relation::resolve_path;
    use crate::query::path::QueryPath;
    use crate::query::{FilterExpr, Operand};

    fn visit_paths(expr: &FilterExpr, visit: &mut impl FnMut(&QueryPath)) {
        match expr {
            FilterExpr::Term(term) => {
                visit(&term.path);
                if let Operand::Field(field) = &term.operand {
                    visit(&field.path);
                }
            }
            FilterExpr::And(parts) | FilterExpr::Or(parts) => {
                for part in parts {
                    visit_paths(part, visit);
                }
            }
            FilterExpr::Not(inner) => visit_paths(inner, visit),
        }
    }

    if query.collections.is_empty() {
        return vec![true; repo.collections.len()];
    }
    let mut scope = vec![false; repo.collections.len()];
    let has_relations = repo
        .collections
        .iter()
        .any(|collection| !collection.definition.schema.relations().is_empty());
    for (index, collection) in repo.collections.iter().enumerate() {
        if !query.collections.contains(&collection.definition.name) {
            continue;
        }
        scope[index] = true;
        if !has_relations {
            continue;
        }
        // The execution binder reports bad paths as query errors.
        let mut visit = |path: &QueryPath| {
            if let Ok(Some((hops, _))) = resolve_path(repo, index, path) {
                for hop in hops {
                    scope[hop.to.0 as usize] = true;
                }
            }
        };
        if let Some(filter) = &query.filter {
            visit_paths(filter, &mut visit);
        }
        if let Some(fts) = &query.fts {
            for field in &fts.fields {
                let path = match field.query_path.as_deref() {
                    Some(path) => QueryPath::parse(path),
                    None => QueryPath::parse(&field.path.display()),
                };
                if let Ok(path) = path {
                    visit(&path);
                }
            }
        }
        for field in &query.options.sort {
            if let Ok(expression) = crate::query::sort_binding::expression(field)
                && let Ok(path) = QueryPath::parse(&expression.path)
            {
                visit(&path);
            }
        }
    }
    scope
}

/// Bind one query against the selected collections on every request.
pub(crate) fn prepare(repo: &Repository, query: &Query) -> Result<PreparedQuery, String> {
    #[cfg(test)]
    PRODUCTION_PREPARE_CALLS.with(|calls| calls.set(calls.get() + 1));
    #[cfg(feature = "detailed-profiling")]
    let started = std::time::Instant::now();
    let mut collections = Vec::new();
    let mut leaves = Vec::new();
    let mut builder = PredicateBuilder::new(0);
    let mut parameter_types = HashMap::new();
    for (index, collection) in repo.collections.iter().enumerate() {
        if !query.collections.is_empty() && !query.collections.contains(&collection.definition.name)
        {
            continue;
        }
        let filter = query
            .filter
            .as_ref()
            .map(|f| {
                crate::query::binding::relation::bind_filter_with_relations(
                    repo,
                    index,
                    f,
                    &mut parameter_types,
                )
            })
            .transpose()?;
        let fts = query
            .fts
            .as_ref()
            .map(|fts| crate::query::binding::relation::bind_fts_with_relations(repo, index, fts))
            .transpose()?;
        let mut collection_leaves = Vec::new();
        let (filter_shape, text_term) =
            builder.prepare(index, filter.as_ref(), fts.as_ref(), &mut collection_leaves);
        leaves.extend(collection_leaves);
        collections.push(PreparedCollectionQuery {
            collection_index: index,
            filter,
            fts,
            filter_shape,
            text_term,
        });
    }
    if collections.is_empty() {
        return Err("unknown or unloaded collection scope".into());
    }

    #[cfg(feature = "detailed-profiling")]
    if std::env::var_os("OBJEKTE_BIND_PHASES").is_some() {
        eprintln!(
            "query-binding ns={} regex={}",
            started.elapsed().as_nanos(),
            query
                .fts
                .as_ref()
                .is_some_and(|fts| matches!(fts.mode, crate::query::FtsMode::Regex(_)))
        );
    }
    Ok(PreparedQuery {
        collections,
        predicates: Arc::new(PreparedPredicates::new(leaves)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::options::ReturnMode;
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery, QueryOptions, RegexOptions};
    use crate::repository::configuration::RepositoryOptions;

    #[test]
    fn each_request_binds_new_regex_and_independent_collections() {
        let root = std::env::temp_dir().join(format!(
            "objekte-fresh-binding-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("first")).unwrap();
        std::fs::create_dir_all(root.join("second")).unwrap();
        std::fs::write(root.join("first/one.row"), br#"("title"="abc")"#).unwrap();
        std::fs::write(root.join("second/two.row"), br#"("title"="def")"#).unwrap();
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            ["first", "second"]
                .into_iter()
                .map(|name| crate::source::test_support::document_folder(name, name))
                .collect(),
            RepositoryOptions::default(),
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let query = Query {
            collections: Vec::new(),
            filter: None,
            fts: Some(FtsQuery {
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Regex(RegexOptions::default()),
                literal: "a+".into(),
                pattern_parameter: None,
                fields: Vec::new(),
            }),
            options: QueryOptions {
                sort: Vec::new(),
                limit: 1,
                offset: 0,
                count: false,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let before = crate::query::regex::test_compile_count();
        let mut first = prepare(&repo, &query).unwrap();
        let second = prepare(&repo, &query).unwrap();
        assert_eq!(first.collections.len(), 2);
        assert_eq!(second.collections.len(), 2);
        assert_eq!(crate::query::regex::test_compile_count() - before, 4);
        first.collections[0].fts.as_mut().unwrap().source_literal = "changed".into();
        assert_eq!(
            second.collections[0].fts.as_ref().unwrap().source_literal,
            "a+"
        );
        assert!(first.collections[0].text_term.is_some());
        assert!(first.collections[0].filter_shape.is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repeated_equal_terms_keep_distinct_request_local_ids() {
        let root = std::env::temp_dir().join(format!(
            "objekte-term-identities-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/one.row"), br#"("title"="same","year"=1)"#).unwrap();
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![crate::source::test_support::document_folder("data", "data")],
            RepositoryOptions::default(),
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let filter = crate::query::parse_filter("title = \"same\" && title = \"other\"").unwrap();
        let query = Query {
            collections: Vec::new(),
            filter: Some(filter),
            fts: None,
            options: QueryOptions {
                sort: Vec::new(),
                limit: 1,
                offset: 0,
                count: false,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let prepared = prepare(&repo, &query).unwrap();
        let entry = &prepared.collections[0];
        let ids = entry.filter_shape.as_ref().unwrap().term_ids();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        let filter = entry.filter.as_ref().unwrap();
        let BoundExpr::And(parts) = filter else {
            unreachable!()
        };
        let BoundExpr::Term(first) = &parts[0] else {
            unreachable!()
        };
        let BoundExpr::Term(second) = &parts[1] else {
            unreachable!()
        };
        assert_ne!(
            first.span, second.span,
            "runtime identity must not be inferred from predicate semantics"
        );
        assert_eq!(
            prepared
                .predicates
                .term_for_bound_term(entry.collection_index, second),
            Some(ids[1])
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
