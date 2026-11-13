use super::*;
use crate::model::RecordSlot;
use crate::segment_format::SegmentWriter;
use crate::source::TextSegment;
use std::io::Cursor;

fn seal(index: &BigramIndex) -> SealedBigramIndex {
    let allowed = Arc::new(RoaringBitmap::from_iter(0..index.record_count));
    let mut pack = PostingPack::new();
    let mut dictionaries = Vec::new();
    let spans = append_bigram_index(&mut dictionaries, &mut pack, index, &allowed).unwrap();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 2).unwrap();
    writer
        .write_section(SectionKind::BigramDictionaries, |out| {
            out.write_all(&dictionaries)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    writer
        .write_section(SectionKind::PostingPayloads, |out| {
            out.write_all(pack.bytes())
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let reader = Arc::new(
        SegmentReader::from_owned(Arc::from(writer.finish().unwrap().into_inner())).unwrap(),
    );
    SealedBigramIndex::open(reader, spans, index.alphabet, index.record_count, allowed).unwrap()
}

#[test]
fn ascii_and_unicode_normal_skip_candidates_match_mutable_index() {
    let records = [
        vec!["hello", "world"],
        vec!["hel", "loworld"],
        vec!["xhelloworldy"],
        vec!["straße and café"],
        vec!["strasse and cafe"],
        vec!["abcd efgh"],
        vec!["acbd efg"],
        vec!["😀alphaβ"],
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut index = BigramIndex::new(alphabet);
        for (slot, parts) in records.iter().enumerate() {
            let segments = parts
                .iter()
                .map(|text| TextSegment {
                    path: vec!["text".into()],
                    text: (*text).into(),
                })
                .collect::<Vec<_>>();
            index
                .add_record(RecordSlot(slot as u32), &segments)
                .unwrap();
        }
        let sealed = seal(&index);
        assert_eq!(sealed.record_count(), index.record_count);
        for literal in [
            "hello",
            "helloworld",
            "abcd",
            "acbd",
            "strasse",
            "straße",
            "café",
            "😀alphaβ",
            "z",
            "zzzz",
            "ü",
            "βa",
        ] {
            let request = BigramRequest::new(literal);
            assert_eq!(
                sealed.candidates_prepared(&request).unwrap(),
                index.candidates_prepared(&request),
                "{alphabet:?} {literal}"
            );
            assert_eq!(
                sealed.estimate_prepared(&request).unwrap(),
                index.estimate_prepared(&request),
                "estimate {alphabet:?} {literal}"
            );
        }
    }
}

#[test]
fn encoding_rejects_postings_outside_binding_domain() {
    let mut index = BigramIndex::new_ascii();
    let text = [TextSegment {
        path: vec!["text".into()],
        text: "abc".into(),
    }];
    index.add_record(RecordSlot(0), &text).unwrap();
    index.add_record(RecordSlot(1), &text).unwrap();
    let mut pack = PostingPack::new();
    assert!(
        append_bigram_index(
            &mut Vec::new(),
            &mut pack,
            &index,
            &RoaringBitmap::from_iter([0]),
        )
        .is_err()
    );
}

#[test]
fn installed_reader_releases_mutable_gram_maps_and_keeps_candidates() {
    let mut index = BigramIndex::new_ascii();
    for (slot, text) in ["abcde", "axcye", "zzzzz"].into_iter().enumerate() {
        index
            .add_record(
                RecordSlot(slot as u32),
                &[TextSegment {
                    path: vec!["text".into()],
                    text: text.into(),
                }],
            )
            .unwrap();
    }
    let request = BigramRequest::new("abc");
    let expected = index.candidates_prepared(&request);
    let allowed = Arc::new(RoaringBitmap::from_iter(0..index.record_count));
    let mut pack = PostingPack::new();
    let mut dictionaries = Vec::new();
    let spans = append_bigram_index(&mut dictionaries, &mut pack, &index, &allowed).unwrap();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 2).unwrap();
    writer
        .write_section(SectionKind::BigramDictionaries, |out| {
            out.write_all(&dictionaries)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    writer
        .write_section(SectionKind::PostingPayloads, |out| {
            out.write_all(pack.bytes())
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let reader = Arc::new(
        SegmentReader::from_owned(Arc::from(writer.finish().unwrap().into_inner())).unwrap(),
    );
    index.install_sealed(reader, spans, allowed).unwrap();
    assert!(index.normal.is_empty() && index.skip.is_empty());
    assert_eq!(index.try_candidates_prepared(&request).unwrap(), expected);
    assert_eq!(index.try_estimate_prepared(&request).unwrap(), Some(1));
}

#[test]
fn presence_filter_preserves_every_sealed_gram() {
    let mut index = BigramIndex::new_ascii();
    for slot in 0..100 {
        let text = format!("prefix{slot:03}suffix");
        index
            .add_record(
                RecordSlot(slot),
                &[TextSegment { path: vec!["text".into()], text }],
            )
            .unwrap();
    }
    let sealed = seal(&index);
    for (dictionary, presence) in [
        (&sealed.normal, &sealed.normal_presence),
        (&sealed.skip, &sealed.skip_presence),
    ] {
        let presence = presence.as_ref().expect("enough grams to build a filter");
        dictionary
            .visit_entries(|key, _| {
                assert!(presence.might_contain(*key));
                Ok(true)
            })
            .unwrap();
    }
    assert_eq!(
        sealed.candidates_prepared(&BigramRequest::new("definitely missing"))
            .unwrap(),
        index.candidates_prepared(&BigramRequest::new("definitely missing"))
    );
}
