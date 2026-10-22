use std::borrow::Cow;

/// Imported Markdown often escapes punctuation that is already literal. Hide
/// those redundant escapes in the source control without changing its meaning.
/// JSON/YAML quoting remains entirely the storage serializer's responsibility.
pub fn readable(source: &str) -> Cow<'_, str> {
    if !source.contains('\\') {
        return Cow::Borrowed(source);
    }
    fn rendered(source: &str) -> String {
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new(source));
        html
    }
    // Compare before sanitizing so source HTML, links and code remain exact.
    let original = rendered(source);
    let mut result = Cow::Borrowed(source);
    let punctuation: std::collections::BTreeSet<char> = source
        .as_bytes()
        .windows(2)
        .filter(|pair| pair[0] == b'\\' && pair[1].is_ascii_punctuation())
        .map(|pair| pair[1] as char)
        .collect();
    for punctuation in punctuation {
        let candidate = result.replace(&format!("\\{punctuation}"), &punctuation.to_string());
        if candidate != result && rendered(&candidate) == original {
            result = Cow::Owned(candidate);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_brackets_are_readable_without_changing_markdown() {
        let imported = r"Der neue Menoza. \[[→](https://ag-historische-stadtkerne.de/jahresthema/sommertheater/)\]";
        assert_eq!(
            readable(imported),
            "Der neue Menoza. [[→](https://ag-historische-stadtkerne.de/jahresthema/sommertheater/)]"
        );
        for meaningful in [
            r"\*literal asterisks\*",
            r"\[literal](https://example.test)",
            "`\\[code\\]`",
            "```text\n\\[code\\]\n```",
            r"C:\Users\name\file.txt",
            "Line one\\\nLine two",
            r"<span data-source='\[x\]'>x</span>",
        ] {
            assert_eq!(readable(meaningful), meaningful, "{meaningful}");
        }
        assert_eq!(
            readable("He said \"hello\"\nSecond line"),
            "He said \"hello\"\nSecond line"
        );
    }
}
