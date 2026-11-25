use crate::query::options::{ReturnMode, SortField};

#[derive(Clone)]
pub struct QueryFixture {
    pub collection: String,
    pub fts: Option<String>,
    pub fts_case_sensitivity: crate::query::CaseSensitivity,
    pub fts_mode: crate::query::FtsMode,
    pub filter: Option<String>,
    pub fts_fields: Vec<String>,
    pub sort: Vec<SortField>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
    pub return_mode: ReturnMode,
    pub evidence: bool,
}

/// Build the typed query for a test fixture.
pub fn query_from_fixture(case: &QueryFixture) -> Result<crate::query::Query, String> {
    use crate::query::{FtsQuery, Query, QueryOptions};
    let fts = case
        .fts
        .as_deref()
        .map(|literal| {
            FtsQuery::from_syntax(
                literal,
                case.fts_case_sensitivity,
                case.fts_mode,
                &case.fts_fields,
            )
        })
        .transpose()?;
    let collections = if case.collection == "*" {
        Vec::new()
    } else {
        vec![case.collection.clone()]
    };
    Query::from_syntax(
        collections,
        fts,
        case.filter.as_deref(),
        QueryOptions {
            sort: case.sort.clone(),
            limit: case.limit,
            offset: case.offset,
            count: case.count,
            return_mode: case.return_mode.clone(),
            evidence: case.evidence,
        },
    )
}
