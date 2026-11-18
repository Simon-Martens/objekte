//! Shared Unicode text normalization for exact matching and gram indexing.

use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

pub(crate) fn fold(text: &str) -> String {
    let mut output = String::new();
    fold_into(text, &mut output);
    output
}

/// Write the built-in full Unicode fold into caller-owned reusable storage.
pub(crate) fn fold_into(text: &str, output: &mut String) {
    output.clear();
    if text.is_ascii() {
        output.push_str(text);
        output.make_ascii_lowercase();
    } else {
        output.extend(folded_chars(text));
    }
}

pub(crate) fn folded_chars(text: &str) -> impl Iterator<Item = char> + '_ {
    text.case_fold_with(Variant::Full, Locale::NonTurkic)
}
