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

    for operator in [Operator::Eq, Operator::Lt] {
        let (value, expected) = match operator {
            Operator::Eq => (json!(1), 3072),
            Operator::Lt => (json!(2), 3072),
            _ => unreachable!(),
        };
        let bits = index.term_bits("score", operator, &value, 4096).unwrap();
        assert_eq!(
            bits.iter().map(|word| word.count_ones()).sum::<u32>(),
            expected
        );
        for slot in 0..4096 {
            assert_eq!(bits[slot / 64] & (1 << (slot % 64)) != 0, slot % 4 != 3);
        }
    }
}
