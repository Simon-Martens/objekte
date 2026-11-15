//! Shared bitmap block codec tests.

use super::*;

fn block_with(offsets: &[u16]) -> Block {
    let mut block = Block::default();
    for offset in offsets {
        block.set(*offset);
    }
    block
}

#[test]
fn cardinality_boundaries_are_canonical() {
    for cardinality in [1usize, 255, 256, 4096] {
        let offsets = (0..cardinality)
            .map(|index| {
                if cardinality == 1 {
                    0
                } else {
                    ((index * 4095) / (cardinality - 1)) as u16
                }
            })
            .collect::<Vec<_>>();
        let block = block_with(&offsets);
        let encoded = encode_block(&block).expect("encode");
        assert_eq!(encoded[1], if cardinality <= 255 { 0 } else { 1 });
        assert_eq!(decode_block(&encoded).expect("decode"), block);
    }
}

#[test]
fn malformed_values_are_rejected() {
    assert!(decode_block(&[]).is_err());
    assert!(decode_block(&[2, 0, 1, 0, 0, 0]).is_err());
    assert!(decode_block(&[1, 0, 0, 0]).is_err());
    assert!(decode_block(&[1, 0, 2, 0, 1, 0, 1, 0]).is_err());
    assert!(decode_block(&[1, 1, 255, 0, 0]).is_err());
}
