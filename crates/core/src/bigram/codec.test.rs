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
