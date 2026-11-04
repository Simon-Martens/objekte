use super::*;


fn pattern(text: &str, mode: TextMode, case_sensitive: bool) -> TextPattern {
    TextPattern::compile(text, mode, case_sensitive).unwrap()
}


#[test]
fn word_mode_reads_normalized_text_and_folds_case() {
    let word = pattern("goethe, johann", TextMode::Word, false);
    assert_eq!(word.needle(), Some("goethe johann"));
    assert!(word.matches("„Goethe“ – Johann", TextFormat::Plain));
    assert!(word.matches("**Goethe**, Johann", TextFormat::Markdown));
    assert!(word.matches("STRASSE GOETHE JOHANN", TextFormat::Plain));
    assert!(!word.matches("GoetheJohann", TextFormat::Plain));
    let sensitive = pattern("Goethe", TextMode::Word, true);
    assert!(!sensitive.matches("goethe", TextFormat::Plain));
    // Unicode folding: ß matches ss.
    assert!(pattern("strasse", TextMode::Word, false).matches("Straße", TextFormat::Plain));
}

#[test]
fn raw_word_and_regex_read_the_stored_text() {
    let raw = pattern("**Goethe**,", TextMode::WordRaw, false);
    assert!(raw.matches("**goethe**, Johann", TextFormat::Markdown));
    assert!(!raw.matches("Goethe Johann", TextFormat::Plain));
    let regex = pattern(r"^\*\*Goethe", TextMode::Regex, false);
    assert!(!regex.folds(), "a regex sets its own case");
    assert!(regex.matches("**Goethe**", TextFormat::Markdown));
    assert!(!regex.matches("**goethe**", TextFormat::Markdown));
    assert!(pattern("(?i)^goethe", TextMode::Regex, false).matches("GOETHE", TextFormat::Plain));
}

#[test]
fn equality_compares_the_whole_text_as_read() {
    let word = pattern("Goethe, Johann", TextMode::Word, false);
    assert!(word.equals("goethe johann", TextFormat::Plain));
    assert!(!word.equals("goethe johann w", TextFormat::Plain));
    let raw = pattern("Goethe, Johann", TextMode::WordRaw, true);
    assert!(raw.equals("Goethe, Johann", TextFormat::Plain));
    assert!(!raw.equals("goethe, johann", TextFormat::Plain));
    let regex = pattern("Goethe.*", TextMode::Regex, false);
    assert!(
        regex.equals("Goethe.*", TextFormat::Plain),
        "exact stored text"
    );
    assert!(!regex.equals("Goethe Johann", TextFormat::Plain));
}

#[test]
fn unusable_patterns_are_rejected() {
    for (text, mode, message) in [
        ("", TextMode::Word, "1–16384 bytes"),
        (", –", TextMode::Word, "letters or digits"),
        ("(", TextMode::Regex, "invalid text regex"),
    ] {
        let error = TextPattern::compile(text, mode, false).unwrap_err();
        assert!(error.to_string().contains(message), "{text}: {error}");
    }
    // Stored-text search can look for punctuation.
    assert!(TextPattern::compile(", –", TextMode::WordRaw, false).is_ok());
}
