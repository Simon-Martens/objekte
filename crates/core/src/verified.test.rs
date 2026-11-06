use super::*;
#[test]
fn bounded_entries_include_complete_empty_results() {
    let mut cache = VerifiedCache::new(300);
    cache.insert("empty".into(), Vec::new());
    assert_eq!(cache.get(b"empty").unwrap().members.len(), 0);
    cache.insert("other".into(), Vec::new());
    cache.insert("third".into(), Vec::new());
    assert!(cache.used_bytes() <= 300);
    assert!(cache.get(b"empty").is_none());
    cache.set_budget(0);
    assert!(!cache.enabled());
    assert!(cache.get(b"other").is_none());
}

#[test]
fn structural_key_distinguishes_bound_values_sort_and_fts_mode() {
    use crate::model::{LogicalType, Value};
    use crate::query::{CaseSensitivity, FtsQuery, QueryOptions, parse_filter};
    use crate::workload::{ReturnMode, SortField};

    let template = Query {
        collections: vec!["data".into()],
        filter: Some(parse_filter("title = $value").unwrap()),
        fts: None,
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let string = template
        .with_parameters(&std::collections::HashMap::from([(
            "value".into(),
            Value::String("2026-11-06".into()),
        )]))
        .unwrap();
    let date = template
        .with_parameters(&std::collections::HashMap::from([(
            "value".into(),
            Value::parse_typed_string("2026-11-06", &LogicalType::Date).unwrap(),
        )]))
        .unwrap();
    assert_ne!(key(&string).unwrap(), key(&date).unwrap());

    let mut sorted = string.clone();
    sorted.options.sort.push(SortField {
        field: "title".into(),
        descending: true,
    });
    assert_ne!(key(&string).unwrap(), key(&sorted).unwrap());
    let mut fts = string.clone();
    fts.fts = Some(FtsQuery {
        pattern_parameter: None,
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: crate::query::FtsMode::Literal,
        literal: "Der".into(),
        fields: vec![],
    });
    let mut insensitive = fts.clone();
    insensitive.fts.as_mut().unwrap().case_sensitivity = CaseSensitivity::Insensitive;
    assert_ne!(key(&fts).unwrap(), key(&insensitive).unwrap());
}
