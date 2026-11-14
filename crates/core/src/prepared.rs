//! Fresh, validated typed query bindings for each request.

use std::collections::HashMap;

use crate::query::Query;
use crate::reference::{BoundExpr, BoundFts};
use crate::repository::Repository;


/// Bound query terms for the selected collection generations.
#[derive(Clone)]
pub(crate) struct PreparedQuery {
    pub collections: Vec<(usize, Option<BoundExpr>, Option<BoundFts>)>,
}


/// Bind one query against the selected collections on every request.
pub(crate) fn prepare(repo: &Repository, query: &Query) -> Result<PreparedQuery, String> {
    let mut collections = Vec::new();
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
        collections.push((index, filter, fts));
    }
    if collections.is_empty() {
        return Err("unknown or unloaded collection scope".into());
    }
    Ok(PreparedQuery { collections })
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
        first.collections[0].2.as_mut().unwrap().source_literal = "changed".into();
        assert_eq!(second.collections[0].2.as_ref().unwrap().source_literal, "a+");
        std::fs::remove_dir_all(root).unwrap();
    }
}
