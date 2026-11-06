use super::*;

#[test]
fn stable_icu_keys_and_fixed_null_placement() {
    let locale: Locale = "de".parse().unwrap();
    let collator = CollatorBorrowed::try_new(locale.into(), CollatorOptions::default()).unwrap();
    let key = |s: &str| {
        let mut out = Vec::new();
        collator.write_sort_key_to(s, &mut out).unwrap();
        SortKey::String(out)
    };
    assert!(key("Ähre").compare(&key("Zug"), false).is_lt());
    assert!(key("Ähre").compare(&key("Zug"), true).is_gt());
    assert!(SortKey::Null.compare(&key("Zug"), true).is_gt());
    assert!(SortKey::Missing.compare(&SortKey::Null, true).is_gt());
    assert!(
        SortKey::Number(Number::integer(9_007_199_254_740_993))
            .compare(
                &SortKey::Number(Number::integer(9_007_199_254_740_992)),
                false
            )
            .is_gt()
    );
}
