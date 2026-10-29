use super::*;

#[test]
fn matching_handles_wrapping_wildcards_escapes_and_ascii_folding() {
    for (pattern, field, text, expected) in [
        ("abc", false, "xxABCyy", true),
        ("abc%", false, "abcdef", true),
        ("abc%", false, "xabcdef", false),
        ("a_c", false, "a🐈c", false),
        ("%a_c%", false, "a🐈c", true),
        (r"a\_c", false, "a_c", true),
        (r"a\%c", false, "a%c", true),
        ("abc%", true, "xxabcxyz", true),
        ("", false, "anything", true),
    ] {
        assert_eq!(
            Like::new(pattern, field).matches(text),
            expected,
            "{pattern:?} {field} {text:?}"
        );
    }
}
