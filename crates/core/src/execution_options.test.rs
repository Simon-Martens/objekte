use super::{BigramAlphabet, ReturnMode, SortExpression, SortField, SortReducer};

#[test]
fn workload_reexports_are_the_runtime_option_types() {
    let mode: crate::workload::ReturnMode = ReturnMode::Logical;
    let alphabet: crate::workload::BigramAlphabet = BigramAlphabet::Unicode;
    let expression: crate::workload::SortExpression = SortExpression {
        path: "title".into(),
        binding: None,
        reducer: Some(SortReducer::First),
    };
    let field: crate::workload::SortField = SortField::computed(expression, false);
    assert_eq!(mode, ReturnMode::Logical);
    assert_eq!(alphabet, BigramAlphabet::Unicode);
    assert_eq!(field.field, "title");
    assert_eq!(serde_json::to_string(&mode).unwrap(), "\"logical\"");
    assert_eq!(serde_json::to_string(&alphabet).unwrap(), "\"unicode\"");
}
