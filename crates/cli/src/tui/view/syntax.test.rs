use super::*;

#[test]
fn tokens_get_their_colours() {
    let text = "year >= 1800 && title ~ 'Moon' SORT BY @modified DESC";
    let styles = filter_styles(text);
    let at = |needle: &str| styles[text.find(needle).unwrap()];
    assert_eq!(at("year").fg, Some(TEXT));
    assert_eq!(at(">=").fg, Some(VIOLET));
    assert_eq!(at("1800").fg, Some(AMBER));
    assert_eq!(at("&&").fg, Some(ACCENT));
    assert_eq!(at("'Moon'").fg, Some(GREEN));
    assert_eq!(
        styles[text.find("Moon'").unwrap() + 4].fg,
        Some(GREEN),
        "closing quote"
    );
    assert_eq!(at("SORT").fg, Some(ACCENT));
    assert_eq!(at("@modified").fg, Some(TEAL));
    assert_eq!(styles.len(), text.chars().count());
}

#[test]
fn unterminated_strings_and_escapes_stay_strings() {
    let text = "a = 'it\\'s";
    let styles = filter_styles(text);
    assert!(styles[4..].iter().all(|style| style.fg == Some(GREEN)));
}

#[test]
fn error_positions_count_characters() {
    assert_eq!(
        error_position("ä = ", "filter at byte 6: expected value"),
        Some(4)
    );
    assert_eq!(error_position("year", "filter at byte 99: x"), Some(4));
    assert_eq!(error_position("year", "unknown"), None);
}
