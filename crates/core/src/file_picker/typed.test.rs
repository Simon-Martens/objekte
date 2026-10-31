use super::*;
use serde_json::json;

#[test]
fn grouped_number_spellings_keep_exact_equality_and_ranges() {
    let mut index = TypedIndex::new(vec!["score".into()]);
    let rows = (0..4096)
        .map(|slot| {
            let raw = match slot % 4 {
                0 => "1",
                1 => "1.0",
                2 => "1e0",
                _ => "2",
            };
            (TypedKey::Number(raw.into()), slot)
        })
        .collect();
    index.replace_rows(vec![rows]);
    assert_eq!(index.postings[0].len(), 2);

    for operator in [IndexOp::Eq, IndexOp::Lt] {
        let (value, expected) = match operator {
            IndexOp::Eq => (json!(1), 3072),
            IndexOp::Lt => (json!(2), 3072),
            _ => unreachable!(),
        };
        let bits = index
            .term_bits(
                &vec![JsonPathStep::Key("score".into())],
                operator,
                &value,
                4096,
            )
            .unwrap();
        assert_eq!(
            bits.iter().map(|word| word.count_ones()).sum::<u32>(),
            expected
        );
        for slot in 0..4096 {
            assert_eq!(bits[slot / 64] & (1 << (slot % 64)) != 0, slot % 4 != 3);
        }
    }
}

#[test]
fn grouped_postings_distinguish_null_and_missing() {
    let mut index = TypedIndex::new(vec!["score".into()]);
    let rows = (0..4096)
        .map(|slot| {
            let key = if slot % 10 == 0 {
                TypedKey::Null
            } else if slot % 10 == 1 {
                TypedKey::Missing
            } else {
                TypedKey::Number("1".into())
            };
            (key, slot)
        })
        .collect();
    index.replace_rows(vec![rows]);
    assert_eq!(index.postings[0][&TypedKey::Null].len(), 410);
    assert_eq!(index.postings[0][&TypedKey::Missing].len(), 410);

    for (operator, value, expected) in [
        (IndexOp::Eq, Value::Null, 410),
        (IndexOp::Ne, json!(1), 410),
        (IndexOp::Ne, Value::Null, 3276),
    ] {
        let bits = index
            .term_bits(
                &vec![JsonPathStep::Key("score".into())],
                operator,
                &value,
                4096,
            )
            .unwrap();
        assert_eq!(
            bits.iter().map(|word| word.count_ones()).sum::<u32>(),
            expected
        );
    }
}
