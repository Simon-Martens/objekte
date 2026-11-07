use super::*;


fn press(input: &mut Input, code: KeyCode, modifiers: KeyModifiers) -> Option<bool> {
    input.key(KeyEvent::new(code, modifiers))
}

fn typed(input: &mut Input, text: &str) {
    for ch in text.chars() {
        press(input, KeyCode::Char(ch), KeyModifiers::NONE);
    }
}

#[test]
fn typing_inserts_at_the_cursor_and_counts_characters() {
    let mut input = Input::default();
    typed(&mut input, "Grüße");
    assert_eq!(input.text, "Grüße");
    assert_eq!(input.cursor, 5);
    press(&mut input, KeyCode::Left, KeyModifiers::NONE);
    press(&mut input, KeyCode::Left, KeyModifiers::NONE);
    typed(&mut input, "x");
    assert_eq!(input.text, "Grüxße");
    assert_eq!(input.cursor, 4);
}

#[test]
fn edits_report_changes_and_moves_do_not() {
    let mut input = Input::with_text("abc");
    assert_eq!(
        press(&mut input, KeyCode::Home, KeyModifiers::NONE),
        Some(false)
    );
    assert_eq!(input.cursor, 0);
    assert_eq!(
        press(&mut input, KeyCode::Delete, KeyModifiers::NONE),
        Some(true)
    );
    assert_eq!(input.text, "bc");
    assert_eq!(
        press(&mut input, KeyCode::End, KeyModifiers::NONE),
        Some(false)
    );
    assert_eq!(
        press(&mut input, KeyCode::Backspace, KeyModifiers::NONE),
        Some(true)
    );
    assert_eq!(input.text, "b");
    // Keys the editor does not use are left to the caller.
    assert_eq!(press(&mut input, KeyCode::Up, KeyModifiers::NONE), None);
    assert_eq!(press(&mut input, KeyCode::Enter, KeyModifiers::NONE), None);
}

#[test]
fn shifted_and_altgr_characters_are_text() {
    let mut input = Input::default();
    press(&mut input, KeyCode::Char('a'), KeyModifiers::SHIFT);
    press(
        &mut input,
        KeyCode::Char('@'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    assert_eq!(input.text, "A@");
}

#[test]
fn word_commands_delete_and_move_by_words() {
    let mut input = Input::with_text("year >= 1800");
    press(&mut input, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(input.text, "year >= ");
    press(&mut input, KeyCode::Left, KeyModifiers::CONTROL);
    assert_eq!(input.cursor, 5);
    press(&mut input, KeyCode::Char('k'), KeyModifiers::CONTROL);
    assert_eq!(input.text, "year ");
    press(&mut input, KeyCode::Char('u'), KeyModifiers::CONTROL);
    assert_eq!(input.text, "");
    assert_eq!(input.cursor, 0);
}

#[test]
fn replace_puts_the_cursor_after_the_inserted_text() {
    let mut input = Input::with_text("ye && x");
    input.replace(0, 2, "year");
    assert_eq!(input.text, "year && x");
    assert_eq!(input.cursor, 4);
}
