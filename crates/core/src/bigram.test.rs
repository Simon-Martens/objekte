use super::*;

#[test]
fn conservative_ascii_candidates_keep_segments_and_records_separate() {
    let mut index = BigramIndex::new_ascii();
    for (slot, parts) in [
        vec!["hello", "world"],
        vec!["hel", "loworld"],
        vec!["xhelloworldy"],
    ]
    .into_iter()
    .enumerate()
    {
        let segments = parts
            .into_iter()
            .map(|s| TextSegment {
                path: vec!["text".into()],
                text: s.into(),
            })
            .collect::<Vec<_>>();
        index
            .add_record(RecordSlot(slot as u32), &segments)
            .unwrap();
    }
    index.validate(3).unwrap();
    let candidates = index.candidates("helloworld").unwrap();
    assert!(candidates.contains(2));
    assert!(!candidates.contains(0));
    assert!(index.candidates("ü").is_none());
    assert!(index.candidates("a").is_none());
    assert!(index.candidates("unseen bigram").unwrap().is_empty());
}

#[test]
fn ascii_and_unicode_candidates_are_conservative() {
    let segments = ["Müller", "Straße", "Leipzig"].map(|text| {
        vec![TextSegment {
            path: vec!["name".into()],
            text: text.into(),
        }]
    });
    let mut ascii = BigramIndex::new(BigramAlphabet::Ascii);
    let mut unicode = BigramIndex::new(BigramAlphabet::Unicode);
    for (slot, record) in segments.iter().enumerate() {
        ascii.add_record(RecordSlot(slot as u32), record).unwrap();
        unicode.add_record(RecordSlot(slot as u32), record).unwrap();
    }
    for literal in ["Müller", "Straße", "Leipzig", "ü", "not there"] {
        for (slot, record) in segments.iter().enumerate() {
            if record[0].text.contains(literal) {
                assert!(
                    ascii
                        .candidates(literal)
                        .is_none_or(|set| set.contains(slot as u32))
                );
                assert!(
                    unicode
                        .candidates(literal)
                        .is_none_or(|set| set.contains(slot as u32))
                );
            }
        }
    }
    assert_eq!(ascii.alphabet, BigramAlphabet::Ascii);
    assert_eq!(unicode.alphabet, BigramAlphabet::Unicode);
    assert!(ascii.candidates("Müller").is_some());
    assert!(ascii.candidates("ü").is_none());
}
