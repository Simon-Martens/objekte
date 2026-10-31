use super::*;

#[test]
fn preserves_raw_text_and_decodes_json_escapes() {
    let text = malformed_search_text(br#"{"note":"M\u006fnd \uD83C\uDF19\n", broken"#);
    assert!(text.contains(r"M\u006fnd"));
    assert!(text.contains("Mond 🌙\n"));
}

#[test]
fn invalid_escapes_remain_searchable() {
    let text = malformed_search_text(br#"broken \uZZZZ and \uD800 and \\u006f"#);
    assert!(text.contains(r"\uZZZZ"));
    assert!(text.contains(r"\uD800"));
    assert!(text.contains(r"\u006f"));
    assert!(malformed_search_text("broken \\é".as_bytes()).contains("\\é"));
    assert!(malformed_search_text(b"bad \xff Mond").contains("Mond"));
}

#[test]
fn arbitrary_source_bytes_never_break_escape_scanning() {
    for seed in 0..512u32 {
        let mut state = seed.wrapping_add(1);
        let bytes = (0..128)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 16) as u8
            })
            .collect::<Vec<_>>();
        malformed_search_text(&bytes);
    }
}
