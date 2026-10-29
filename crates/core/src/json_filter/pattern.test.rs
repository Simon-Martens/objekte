use super::*;

#[test]
fn matching_handles_wrapping_wildcards_escapes_and_optional_unicode_folding() {
    for (pattern, field, ignore_case, text, expected) in [
        ("abc", false, false, "xxABCyy", false),
        ("abc", false, true, "xxABCyy", true),
        ("abc%", false, false, "abcdef", true),
        ("abc%", false, false, "xabcdef", false),
        ("a_c", false, false, "a🐈c", false),
        ("%a_c%", false, false, "a🐈c", true),
        (r"a\_c", false, false, "a_c", true),
        (r"a\%c", false, false, "a%c", true),
        ("abc%", true, false, "xxabcxyz", true),
        ("abc%", true, true, "xxABCxyz", true),
        ("Ä", false, true, "ä", true),
        ("Ä", false, false, "ä", false),
        ("STRASSE", false, true, "Straße", true),
        ("Σ", false, true, "ς", true),
        ("", false, false, "anything", true),
    ] {
        assert_eq!(
            Like::new(pattern, field, ignore_case).matches(text),
            expected,
            "{pattern:?} {field} {ignore_case} {text:?}"
        );
    }
}
