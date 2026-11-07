use super::*;

#[test]
fn decoded_scalar_text_uses_values_not_names_or_container_syntax() {
    let bytes = br#"{"name_only": ["a\n", 1.0, 1e0, -0, true, false, null, {"v":"\u00df"}]}"#;
    let all = scalar_segments(bytes, false).unwrap();
    assert_eq!(
        all.iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["a\n", "1.0", "1e0", "-0", "true", "false", "null", "ß"]
    );
    assert!(
        all.iter()
            .all(|segment| segment.path.first().is_some_and(|part| part == "name_only"))
    );
}

#[test]
fn all_text_keeps_duplicate_members_but_selected_text_uses_last_wins() {
    let bytes = br#"{"title":"old","title":{"nested":"new"},"array":[1,"two"]}"#;
    let all = scalar_segments(bytes, false).unwrap();
    assert_eq!(
        all.iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["old", "new", "1", "two"]
    );
    let resolved = scalar_segments(bytes, true).unwrap();
    assert_eq!(
        resolved
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>(),
        ["new", "1", "two"]
    );
}
