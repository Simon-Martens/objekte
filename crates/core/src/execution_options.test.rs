use super::{BigramAlphabet, ReturnMode, SortExpression, SortField, SortReducer};

#[test]
fn workload_reexports_are_the_runtime_option_types() {
    let mode: crate::execution_options::ReturnMode = ReturnMode::Logical;
    let alphabet: crate::execution_options::BigramAlphabet = BigramAlphabet::Unicode;
    let expression: crate::execution_options::SortExpression = SortExpression {
        path: "title".into(),
        binding: None,
        reducer: Some(SortReducer::First),
    };
    let field: crate::execution_options::SortField = SortField::computed(expression, false);
    assert_eq!(mode, ReturnMode::Logical);
    assert_eq!(alphabet, BigramAlphabet::Unicode);
    assert_eq!(field.field, "title");
    assert_eq!(serde_json::to_string(&mode).unwrap(), "\"logical\"");
    assert_eq!(serde_json::to_string(&alphabet).unwrap(), "\"unicode\"");
}
