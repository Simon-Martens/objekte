//! Fresh, validated typed query bindings for each request.

use std::collections::HashMap;

use crate::query::Query;
use crate::query_predicate::{
    PredicateBuilder, PreparedExpr, PreparedLeaf, PreparedPredicates, TermId,
};
use crate::reference::{BoundExpr, BoundFts};
use crate::repository::Repository;

/// One named, fully bound collection entry. The first `prepare` call installs
/// every syntactic leaf and its request-local runtime identifier.
#[derive(Clone)]
pub(crate) struct PreparedCollectionQuery {
    pub(crate) collection_index: usize,
    pub(crate) filter: Option<BoundExpr>,
    pub(crate) fts: Option<BoundFts>,
    pub(crate) filter_shape: Option<PreparedExpr>,
    pub(crate) filter_leaves: Vec<PreparedLeaf>,
    pub(crate) text_term: Option<TermId>,
}

impl PreparedCollectionQuery {
    pub(crate) fn filter(&self) -> Option<&BoundExpr> {
        self.filter.as_ref()
    }

    pub(crate) fn fts(&self) -> Option<&BoundFts> {
        self.fts.as_ref()
    }
}

/// Bound query terms for the selected collection generations.
#[derive(Clone)]
pub(crate) struct PreparedQuery {
    pub(crate) collections: Vec<PreparedCollectionQuery>,
    pub(crate) predicates: PreparedPredicates,
}

/// Bind one query against the selected collections on every request.
pub(crate) fn prepare(repo: &Repository, query: &Query) -> Result<PreparedQuery, String> {
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
                crate::relation_query::bind_filter_with_relations(
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
            .map(|fts| crate::relation_query::bind_fts_with_relations(repo, index, fts))
            .transpose()?;
        let mut collection_leaves = Vec::new();
        let (filter_shape, text_term) = builder.prepare(
            index,
            filter.as_ref(),
            fts.is_some(),
            &mut collection_leaves,
        );
        leaves.extend(collection_leaves.iter().cloned());
        collections.push(PreparedCollectionQuery {
            collection_index: index,
            filter,
            fts,
            filter_shape,
            filter_leaves: collection_leaves,
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
        predicates: PreparedPredicates::new(leaves),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery, QueryOptions, RegexOptions};
    use crate::repository::{CollectionDefinition, RepositoryOptions};
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    #[test]
    fn each_request_binds_new_regex_and_independent_collections() {
        let root = std::env::temp_dir().join(format!(
            "objekte-fresh-binding-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(root.join("first")).unwrap();
        std::fs::create_dir_all(root.join("second")).unwrap();
        std::fs::write(root.join("first/one.json"), br#"{"title":"abc"}"#).unwrap();
        std::fs::write(root.join("second/two.json"), br#"{"title":"def"}"#).unwrap();
        let repo = Repository::open_with_options(
            &root,
            ["first", "second"]
                .into_iter()
                .map(|name| CollectionDefinition::folder(name, name, SourceFormat::Json))
                .collect(),
            RepositoryOptions::default(),
        )
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
        let before = crate::regex_query::test_compile_count();
        let mut first = prepare(&repo, &query).unwrap();
        let second = prepare(&repo, &query).unwrap();
        assert_eq!(first.collections.len(), 2);
        assert_eq!(second.collections.len(), 2);
        assert_eq!(crate::regex_query::test_compile_count() - before, 4);
        first.collections[0].fts.as_mut().unwrap().source_literal = "changed".into();
        assert_eq!(
            second.collections[0].fts.as_ref().unwrap().source_literal,
            "a+"
        );
        assert_eq!(first.collections[0].filter_leaves.len(), 1);
        assert_eq!(
            first.collections[0].filter_leaves[0].id,
            first.collections[0].text_term.unwrap()
        );
        assert!(first.collections[0].text_term.is_some());
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
        std::fs::write(root.join("data/one.json"), br#"{"title":"same","year":1}"#).unwrap();
        let repo = Repository::open_with_options(
            &root,
            vec![CollectionDefinition::folder(
                "data",
                "data",
                SourceFormat::Json,
            )],
            RepositoryOptions::default(),
        )
        .unwrap();
        let filter = crate::query::parse_filter("title = \"same\" && title = \"same\"").unwrap();
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
        assert_eq!(entry.filter_leaves.len(), 2);
        assert_ne!(entry.filter_leaves[0].id, entry.filter_leaves[1].id);
        assert_ne!(
            entry.filter_leaves[0].span, entry.filter_leaves[1].span,
            "runtime identity must not be inferred from predicate semantics"
        );
        assert_eq!(
            prepared
                .predicates
                .term_for_span(entry.collection_index, entry.filter_leaves[1].span),
            Some(entry.filter_leaves[1].id)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
