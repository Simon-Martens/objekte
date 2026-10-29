use super::*;

#[test]
fn matching_agrees_with_sqlite_like_after_pocketbase_wrapping() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    for (input, field, sql_pattern) in [
        ("abc", false, "%abc%"),
        ("abc%", false, "abc%"),
        ("a_c", false, r"%a\_c%"),
        ("%a_c%", false, "%a_c%"),
        (r"\%", false, r"%\%%"),
        (r"\\", false, r"%\\%"),
        (r"a\b", false, r"%a\\b%"),
        (r"a\%", false, r"%a\%%"),
        (r"%a\_%", false, r"%a\_%"),
        (r"%a\", false, r"%a\"),
        ("abc%", true, "%abc%%"),
        ("a_c", true, "%a_c%"),
        ("", false, "%%"),
        ("%_%_%", false, "%_%_%"),
    ] {
        for text in [
            "", "abc", "ABC", "xxabc", "abcxy", "abcc", "a_c", "aXc", "a🐈c", "a%c", "a\\b", "a%b",
            "ÄBC", "äbc",
        ] {
            let expected: bool = db
                .query_row(
                    "SELECT ?1 LIKE ?2 ESCAPE '\\'",
                    rusqlite::params![text, sql_pattern],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                Like::new(input, field).matches(text),
                expected,
                "pattern={input:?} field={field} text={text:?}"
            );
        }
    }
}
