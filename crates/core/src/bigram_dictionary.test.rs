use super::*;
use crate::model::RecordSlot;
use crate::segment_format::SegmentWriter;
use crate::source::TextSegment;
use std::io::{Cursor, Write};

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
