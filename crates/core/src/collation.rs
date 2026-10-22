//! One Unicode collation policy for lists, navigation and picker labels.
use icu_collator::{
    Collator, CollatorBorrowed,
    options::{CollatorOptions, Strength},
};
use std::{cmp::Ordering, sync::LazyLock};

static ALPHABETIC: LazyLock<CollatorBorrowed<'static>> = LazyLock::new(|| {
    let mut options = CollatorOptions::default();
    // Unicode root ordering: accents are secondary; case is not significant.
    // Canonically equivalent composed/decomposed strings compare equally.
    options.strength = Some(Strength::Secondary);
    Collator::try_new(Default::default(), options).expect("bundled Unicode collation data")
});

pub fn alphabetic_cmp(a: &str, b: &str) -> Ordering {
    ALPHABETIC.compare(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_accents_case_and_canonical_equivalence() {
        assert_eq!(
            alphabetic_cmp("Österreich", "O\u{308}sterreich"),
            Ordering::Equal
        );
        assert_eq!(alphabetic_cmp("Édition", "édition"), Ordering::Equal);
        let mut labels = [
            "Zürich",
            "Österreich",
            "Auzeville",
            "Édition",
            "Berlin",
            "Ägypten",
        ];
        labels.sort_by(|a, b| alphabetic_cmp(a, b));
        assert_eq!(
            labels,
            [
                "Ägypten",
                "Auzeville",
                "Berlin",
                "Édition",
                "Österreich",
                "Zürich"
            ]
        );
    }
}
