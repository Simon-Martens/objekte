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

#[test]
fn seeded_substrings_never_escape_declared_bigram_candidates() {
    const SEED: u64 = 0xB16A_A11F_2026_1106;
    let mut state = SEED;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let alphabet = ['a', 'b', ' ', '-', 'é', '中', '🙂', '\u{0301}', '%'];
    let mut records = Vec::new();
    for _ in 0..128 {
        let mut segments = Vec::new();
        for _ in 0..1 + next() as usize % 3 {
            let len = next() as usize % 20;
            let text = (0..len)
                .map(|_| alphabet[next() as usize % alphabet.len()])
                .collect::<String>();
            segments.push(TextSegment {
                path: vec!["text".into()],
                text,
            });
        }
        records.push(segments);
    }
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut index = BigramIndex::new(alphabet);
        for (slot, segments) in records.iter().enumerate() {
            index.add_record(RecordSlot(slot as u32), segments).unwrap();
        }
        for (slot, segments) in records.iter().enumerate() {
            for segment in segments {
                let boundaries = segment
                    .text
                    .char_indices()
                    .map(|(index, _)| index)
                    .chain(std::iter::once(segment.text.len()))
                    .collect::<Vec<_>>();
                for start in 0..boundaries.len() {
                    for end in start + 1..boundaries.len() {
                        let literal = &segment.text[boundaries[start]..boundaries[end]];
                        assert!(
                            index
                                .candidates(literal)
                                .is_none_or(|set| set.contains(slot as u32)),
                            "seed={SEED:#x} alphabet={alphabet:?} slot={slot} literal={literal:?} segment={:?}",
                            segment.text
                        );
                    }
                }
            }
        }
    }
}
