use super::*;

#[test]
fn osc52_encodes_unicode_and_control_characters_as_data() {
    assert_eq!(osc52(""), "\x1b]52;c;\x07");
    assert_eq!(osc52("A"), "\x1b]52;c;QQ==\x07");
    assert_eq!(osc52("AB"), "\x1b]52;c;QUI=\x07");
    assert_eq!(osc52("Müller\x1b"), "\x1b]52;c;TcO8bGxlchs=\x07");
}
