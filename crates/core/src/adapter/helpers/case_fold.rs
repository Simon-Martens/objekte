//! Shared Unicode text normalization for exact matching and gram indexing.

use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

/// Return the full, non-Turkic Unicode case fold of `text`.
pub fn fold(text: &str) -> String {
    let mut output = String::new();
    fold_into(text, &mut output);
    output
}

/// Write the full Unicode fold into caller-owned reusable storage.
///
/// This clears `output` while retaining its allocation for the next call.
pub fn fold_into(text: &str, output: &mut String) {
    output.clear();
    if text.is_ascii() {
        output.push_str(text);
        output.make_ascii_lowercase();
    } else {
        output.extend(folded_chars(text));
    }
}

/// Iterate over folded characters without allocating a result string.
pub fn folded_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.case_fold_with(Variant::Full, Locale::NonTurkic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_expansion_and_reused_storage() {
        let mut scratch = String::with_capacity(64);
        let capacity = scratch.capacity();
        fold_into("STRAßE", &mut scratch);
        assert_eq!(scratch, "strasse");
        assert_eq!(scratch.capacity(), capacity);
        fold_into("ASCII", &mut scratch);
        assert_eq!(scratch, "ascii");
        assert_eq!(fold("Straße"), "strasse");
        assert_eq!(folded_chars("Straße").collect::<String>(), "strasse");
    }
}
