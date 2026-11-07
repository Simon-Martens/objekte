use super::*;

fn independent_index(records: &[Vec<String>], alphabet: BigramAlphabet, base: u32) -> BigramIndex {
    let mut index = BigramIndex::new(alphabet);
    index.record_count = base + records.len() as u32;
    for (offset, segments) in records.iter().enumerate() {
        let slot = base + offset as u32;
        let mut normal = HashSet::new();
        let mut skip = HashSet::new();
        for text in segments {
            let folded = fold(text);
            match alphabet {
                BigramAlphabet::Ascii => {
                    let bytes = folded.as_bytes();
                    for pair in bytes.windows(2) {
                        if let Some(key) = ascii_pair(pair[0], pair[1]) {
                            normal.insert(key);
                        }
                    }
                    for triple in bytes.windows(3) {
                        if let Some(key) = ascii_pair(triple[0], triple[2]) {
                            skip.insert(key);
                        }
                    }
                }
                BigramAlphabet::Unicode => {
                    let chars = folded.chars().collect::<Vec<_>>();
                    for pair in chars.windows(2) {
                        normal.insert(unicode_pair(pair[0], pair[1]));
                    }
                    for triple in chars.windows(3) {
                        skip.insert(unicode_pair(triple[0], triple[2]));
                    }
                }
            }
        }
        for key in normal {
            index.normal.entry(key).or_default().insert(slot);
        }
        for key in skip {
            index.skip.entry(key).or_default().insert(slot);
        }
    }
    index
}

#[test]
fn chunked_builder_matches_independent_complete_posting_maps() {
    let patterns = [
        "ab",
        "cd",
        "aßb",
        "Straße",
        "İIı",
        "Σςσ",
        "aüb",
        "🙂x🙂",
        "中文",
        "aaab",
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        for base in [0u32, 3, 65] {
            for len in [0usize, 1, 63, 64, 65, 127, 128, 129] {
                let mut state = 0xB16A_2026_1106u64;
                let generated = ['a', 'B', 'ß', 'ü', '中', '🙂', 'Σ', ' '];
                let records = (0..len)
                    .map(|offset| {
                        if offset % 11 == 0 {
                            Vec::new()
                        } else if offset % 7 == 0 {
                            vec!["ab".to_owned(), "cd".to_owned()]
                        } else if offset % 5 == 0 {
                            let text = (0..23)
                                .map(|_| {
                                    state ^= state << 13;
                                    state ^= state >> 7;
                                    state ^= state << 17;
                                    generated[state as usize % generated.len()]
                                })
                                .collect::<String>();
                            vec![text]
                        } else {
                            vec![
                                patterns[offset % patterns.len()].to_owned(),
                                patterns[(offset * 3 + 1) % patterns.len()].to_owned(),
                            ]
                        }
                    })
                    .collect::<Vec<_>>();
                let mut builder = BigramBuilder::new_at(alphabet, base);
                for (offset, segments) in records.iter().enumerate() {
                    let slot = RecordSlot(base + offset as u32);
                    if segments.is_empty() {
                        builder.add_empty(slot).unwrap();
                    } else {
                        builder
                            .add_record(slot, segments.iter().map(String::as_str))
                            .unwrap();
                    }
                }
                let built = builder.finish().unwrap();
                let expected = independent_index(&records, alphabet, base);
                assert_eq!(built.record_count, expected.record_count);
                assert_eq!(
                    built.normal, expected.normal,
                    "normal {alphabet:?} base={base} len={len}"
                );
                assert_eq!(
                    built.skip, expected.skip,
                    "skip {alphabet:?} base={base} len={len}"
                );
            }
        }
    }
    let mut boundary = BigramBuilder::new(BigramAlphabet::Unicode);
    boundary.add_record(RecordSlot(0), ["ab", "cd"]).unwrap();
    let boundary = boundary.finish().unwrap();
    assert!(!boundary.normal.contains_key(&unicode_pair('b', 'c')));
    assert!(!boundary.skip.contains_key(&unicode_pair('a', 'c')));
}

#[test]
fn chunked_builder_checks_slot_arithmetic_before_publication() {
    let mut builder = BigramBuilder::new_at(BigramAlphabet::Ascii, u32::MAX - 1);
    builder
        .add_record(RecordSlot(u32::MAX - 1), ["ab"])
        .unwrap();
    assert!(builder.add_empty(RecordSlot(u32::MAX)).is_err());
    let index = builder.finish().unwrap();
    assert!(index.normal[&ascii_pair(b'a', b'b').unwrap()].contains(u32::MAX - 1));
    assert_eq!(index.record_count, u32::MAX);
}

#[test]
fn shifted_blocks_match_incremental_bitmaps_at_chunk_and_container_boundaries() {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let patterns: &[&[&str]] = &[&["ab", "cd"], &["aßb"], &["Straße", "Müller"], &[""], &[]];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        for base in [0u32, 3, 65_535] {
            for length in [0usize, 63, 64, 65, 511, 512, 513] {
                let mut blocks = BigramContribution::new(alphabet);
                blocks.record_count = base;
                let mut old = BigramIndex::new(alphabet);
                old.record_count = base;
                let mut offset = 0;
                while offset < length {
                    let end = (offset + 57).min(length);
                    let mut local = BigramBuilder::new(alphabet);
                    let mut reference = BigramBuilder::new(alphabet);
                    for slot in offset..end {
                        let segments = patterns[slot % patterns.len()];
                        local
                            .add_record(
                                RecordSlot((slot - offset) as u32),
                                segments.iter().copied(),
                            )
                            .unwrap();
                        reference
                            .add_record(
                                RecordSlot((slot - offset) as u32),
                                segments.iter().copied(),
                            )
                            .unwrap();
                    }
                    let at = base + offset as u32;
                    blocks
                        .merge_shifted(local.finish_blocks().unwrap(), at)
                        .unwrap();
                    old.merge_shifted(reference.finish().unwrap(), at).unwrap();
                    offset = end;
                }
                let built = blocks.into_index_in_pool(&pool).unwrap();
                assert_eq!(built.record_count, old.record_count);
                assert_eq!(
                    built.normal, old.normal,
                    "normal {alphabet:?} base={base} len={length}"
                );
                assert_eq!(
                    built.skip, old.skip,
                    "skip {alphabet:?} base={base} len={length}"
                );
                for literal in ["ab", "bc", "aßb", "STRASSE", "MÜLLER", "missing"] {
                    assert_eq!(built.candidates(literal), old.candidates(literal));
                }
            }
        }
    }
}

#[test]
fn block_admission_rejects_coverage_alphabet_and_slot_overflow() {
    let mut global = BigramContribution::new(BigramAlphabet::Ascii);
    let mut local = BigramBuilder::new(BigramAlphabet::Ascii);
    local.add_record(RecordSlot(0), ["ab"]).unwrap();
    assert!(
        global
            .merge_shifted(local.finish_blocks().unwrap(), 1)
            .is_err()
    );
    let mut unicode = BigramBuilder::new(BigramAlphabet::Unicode);
    unicode.add_record(RecordSlot(0), ["ab"]).unwrap();
    assert!(
        global
            .merge_shifted(unicode.finish_blocks().unwrap(), 0)
            .is_err()
    );
    global.record_count = u32::MAX - 1;
    let mut local = BigramBuilder::new(BigramAlphabet::Ascii);
    local.add_record(RecordSlot(0), ["ab"]).unwrap();
    local.add_record(RecordSlot(1), ["cd"]).unwrap();
    assert!(
        global
            .merge_shifted(local.finish_blocks().unwrap(), u32::MAX - 1)
            .is_err()
    );
}

#[test]
fn repository_bulk_path_matches_raw_custom_and_fallback_gram_maps() {
    use crate::model::LogicalType;
    use crate::reference::{CustomBinding, FieldDefinition, Schema, Transform};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    let root = std::env::temp_dir().join(format!("objekte-bulk-gram-maps-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/mixed.jsonl"),
        b"{\"title\":\"a\\u00dfb\"}\n{\"title\":\n{\"title\":\"ab\"}\n{\"title\":null}\n",
    )
    .unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    schema
        .register_custom(
            vec!["title".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    let definition = CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    };
    let raw_records = vec![vec!["aßb".into()], vec![], vec!["ab".into()], vec![]];
    let fallback_records = vec![vec![], vec!["{\"title\":".into()], vec![], vec![]];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let repo = Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                bigram_alphabet: alphabet,
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        let collection = &repo.collections()[0];
        assert_eq!(collection.generation.records.len(), 4);
        let expected_raw = independent_index(&raw_records, alphabet, 0);
        let expected_fallback = independent_index(&fallback_records, alphabet, 0);
        assert_eq!(collection.raw.normal, expected_raw.normal);
        assert_eq!(collection.raw.skip, expected_raw.skip);
        assert_eq!(collection.fallback_text.normal, expected_fallback.normal);
        assert_eq!(collection.fallback_text.skip, expected_fallback.skip);
        assert_eq!(collection.custom_text.len(), 1);
        let custom = collection.custom_text.values().next().unwrap();
        assert_eq!(custom.normal, expected_raw.normal);
        assert_eq!(custom.skip, expected_raw.skip);
    }
    std::fs::remove_dir_all(root).unwrap();
}

/// Manual allocation diagnostic: run this exact ignored test under Valgrind
/// with OBJEKTE_BIGRAM_PROFILE_MODE=convenience or bulk. Fixture preparation
/// is identical and happens before the measured constructor call.
#[test]
#[ignore]
fn profile_single_worker_constructors() {
    let mode = std::env::var("OBJEKTE_BIGRAM_PROFILE_MODE").unwrap_or_else(|_| "bulk".into());
    let alphabet = match std::env::var("OBJEKTE_BIGRAM_PROFILE_ALPHABET").as_deref() {
        Ok("unicode") => BigramAlphabet::Unicode,
        _ => BigramAlphabet::Ascii,
    };
    let patterns = [
        "Straße und Müller",
        "İΣς aüb",
        "Goethe Liebe Der",
        "中文🙂aßb",
        "ab cd ab",
    ];
    let records = (0..10_000usize)
        .map(|index| {
            vec![
                TextSegment {
                    path: vec!["body".into()],
                    text: patterns[index % patterns.len()].into(),
                },
                TextSegment {
                    path: vec!["title".into()],
                    text: patterns[(index * 3 + 1) % patterns.len()].into(),
                },
            ]
        })
        .collect::<Vec<_>>();
    let started = std::time::Instant::now();
    let index = if mode == "convenience" {
        let mut index = BigramIndex::new(alphabet);
        for (slot, segments) in records.iter().enumerate() {
            index.add_record(RecordSlot(slot as u32), segments).unwrap();
        }
        index
    } else {
        assert_eq!(mode, "bulk");
        let mut builder = BigramBuilder::new(alphabet);
        for (slot, segments) in records.iter().enumerate() {
            builder
                .add_record(
                    RecordSlot(slot as u32),
                    segments.iter().map(|segment| segment.text.as_str()),
                )
                .unwrap();
        }
        builder.finish().unwrap()
    };
    println!(
        "mode={mode} alphabet={alphabet:?} elapsed_ns={} keys={} heap_bytes={}",
        started.elapsed().as_nanos(),
        index.normal.len() + index.skip.len(),
        index.estimated_heap_bytes()
    );
    assert_eq!(index.record_count, 10_000);
}

#[test]
fn full_non_turkic_fold_precedes_all_gram_windows() {
    assert_eq!(fold("Straße Σ ς İ I ı"), "strasse σ σ i\u{307} i ı");
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut index = BigramIndex::new(alphabet);
        let segments = vec![TextSegment {
            path: vec!["name".into()],
            text: "aßb".into(),
        }];
        index.add_record(RecordSlot(0), &segments).unwrap();
        for literal in ["aßb", "assb", "SS", "ß", "sB"] {
            assert!(
                index
                    .candidates(literal)
                    .is_some_and(|slots| slots.contains(0)),
                "{alphabet:?} {literal:?}"
            );
        }
        assert!(index.normal.contains_key(&match alphabet {
            BigramAlphabet::Ascii => ascii_pair(b's', b's').unwrap(),
            BigramAlphabet::Unicode => unicode_pair('s', 's'),
        }));
        assert!(index.skip.contains_key(&match alphabet {
            BigramAlphabet::Ascii => ascii_pair(b'a', b's').unwrap(),
            BigramAlphabet::Unicode => unicode_pair('a', 's'),
        }));
    }
}

#[test]
fn unsupported_ascii_bytes_keep_their_window_positions() {
    let mut ascii = BigramIndex::new_ascii();
    ascii
        .add_record(
            RecordSlot(0),
            &[TextSegment {
                path: vec![],
                text: "aüb".into(),
            }],
        )
        .unwrap();
    assert!(ascii.candidates("aüb").is_none());
    assert!(!ascii.normal.contains_key(&ascii_pair(b'a', b'b').unwrap()));
    assert!(!ascii.skip.contains_key(&ascii_pair(b'a', b'b').unwrap()));
    let mut unicode = BigramIndex::new(BigramAlphabet::Unicode);
    unicode
        .add_record(
            RecordSlot(0),
            &[TextSegment {
                path: vec![],
                text: "aüb".into(),
            }],
        )
        .unwrap();
    assert!(unicode.candidates("AÜB").unwrap().contains(0));
    assert!(unicode.candidates("ü").is_none());
}

#[test]
fn explicit_reference_fold_matches_stay_inside_candidate_supersets() {
    fn reference_fold(text: &str) -> String {
        let mut result = String::new();
        for ch in text.chars() {
            match ch {
                'ß' => result.push_str("ss"),
                'Σ' | 'ς' => result.push('σ'),
                'İ' => result.push_str("i\u{307}"),
                'I' => result.push('i'),
                'ı' => result.push('ı'),
                _ if ch.is_ascii_uppercase() => result.push(ch.to_ascii_lowercase()),
                _ => result.push(ch),
            }
        }
        result
    }
    let texts = [
        "Der",
        "der",
        "Straße",
        "STRASSE",
        "aßb",
        "aüb",
        "Σσς",
        "Iİı",
        "e\u{301}",
        "é",
        "🙂a🙂b",
        "中文",
        "ab",
        "cd",
    ];
    let literals = [
        "Der", "der", "DER", "ß", "ss", "assb", "AßB", "aüb", "ab", "b", "Σ", "σ", "ς", "I", "i",
        "İ", "ı", "i\u{307}", "é", "e\u{301}", "🙂", "中文", "bc", "",
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut index = BigramIndex::new(alphabet);
        for (slot, text) in texts.iter().enumerate() {
            index
                .add_record(
                    RecordSlot(slot as u32),
                    &[TextSegment {
                        path: vec![],
                        text: (*text).into(),
                    }],
                )
                .unwrap();
        }
        for (slot, text) in texts.iter().enumerate() {
            for literal in literals {
                if text.contains(literal) || reference_fold(text).contains(&reference_fold(literal))
                {
                    assert!(
                        index
                            .candidates(literal)
                            .is_none_or(|set| set.contains(slot as u32)),
                        "{alphabet:?} text={text:?} literal={literal:?}"
                    );
                }
            }
        }
    }
}

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
