use super::*;
use crate::bigram::build_bigram_index_from_content_with_encoding;

#[test]
fn streaming_bytes_match_in_memory_encoding_for_both_modes() {
    for encoding in [BigramEncoding::Ascii, BigramEncoding::Utf8Bytes] {
        let documents = (0..4096)
            .map(|slot| {
                let mut text = if encoding == BigramEncoding::Ascii {
                    b"ordinary text ".to_vec()
                } else {
                    "Müller 中文 😀 ".as_bytes().to_vec()
                };
                if slot % 2 == 0 {
                    text.extend_from_slice(b"ab");
                }
                if slot % 20 == 0 {
                    text.extend_from_slice("日本語 zq".as_bytes());
                }
                text
            })
            .collect::<Vec<_>>();
        let refs = documents
            .iter()
            .map(|document| Some(document.as_slice()))
            .collect::<Vec<_>>();
        let filter = build_bigram_index_from_content_with_encoding(&refs, encoding);
        assert!(filter.dense_count() > 0);
        assert!(filter.sparse_count() > 0);
        assert!(filter.skip_index().is_some());

        let expected = filter.to_bytes().unwrap();
        assert_eq!(filter.encoded_len().unwrap(), expected.len() as u64);
        let mut streamed = Vec::new();
        filter.write_to(&mut streamed).unwrap();
        assert_eq!(streamed, expected);
        let restored =
            BigramFilter::read_from(&mut streamed.as_slice(), streamed.len() as u64).unwrap();
        for term in ["ab", "zq", "ordinary", "Müller", "中文", "日本語", "absent"] {
            assert_eq!(
                restored.query(term.as_bytes()),
                filter.query(term.as_bytes())
            );
        }
        assert!(
            BigramFilter::read_from(&mut streamed.as_slice(), (streamed.len() - 1) as u64).is_err()
        );
        streamed.push(0);
        assert!(BigramFilter::from_bytes(&streamed).is_err());
    }
}

#[test]
fn sparse_validation_fast_path_keeps_all_bounds_checks() {
    assert!(sparse_valid(&[], 0));
    assert!(sparse_valid(&[0, 1, 1], 4));
    assert!(sparse_valid(&[0x82, 0x01], 131));
    assert!(!sparse_valid(&[0, 0], 4));
    assert!(!sparse_valid(&[4], 4));
    assert!(!sparse_valid(&[0x80], 100));
    assert!(!sparse_valid(
        &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02],
        usize::MAX
    ));
}

#[test]
fn sparse_validation_matches_previous_decoder_on_malformed_sequences() {
    fn reference(data: &[u8], file_count: usize) -> bool {
        let mut at = 0;
        let mut previous = 0u64;
        let mut first = true;
        while at < data.len() {
            let mut gap = 0u64;
            let mut shift = 0;
            loop {
                let Some(&byte) = data.get(at) else {
                    return false;
                };
                at += 1;
                let part = u64::from(byte & 0x7f);
                if shift >= 64 || part > (u64::MAX >> shift) {
                    return false;
                }
                gap |= part << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
            }
            if !first && gap == 0 {
                return false;
            }
            let Some(position) = previous.checked_add(gap) else {
                return false;
            };
            if position >= file_count as u64 {
                return false;
            }
            previous = position;
            first = false;
        }
        true
    }

    let mut state = 0x1234_5678_9abc_def0_u64;
    for length in 0..128 {
        for _ in 0..32 {
            let bytes = (0..length)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect::<Vec<_>>();
            let file_count = state as usize % 10_000;
            assert_eq!(
                sparse_valid(&bytes, file_count),
                reference(&bytes, file_count)
            );
        }
    }
}
