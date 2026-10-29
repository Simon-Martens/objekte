use super::*;

fn partitions(sizes: impl IntoIterator<Item = u64>) -> Vec<(usize, u64)> {
    let files = sizes
        .into_iter()
        .enumerate()
        .map(|(i, size)| {
            (
                format!("r{i:04}"),
                Stamp {
                    size,
                    modified: String::new(),
                    identity: String::new(),
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut remaining = files.iter().peekable();
    let mut batches = Vec::new();
    while remaining.peek().is_some() {
        let batch = next_ingest_batch(&mut remaining);
        let bytes = batch.iter().map(|(_, stamp)| stamp.size).sum();
        assert!(batch.len() <= 256);
        assert!(bytes <= 8 * 1024 * 1024 || batch.len() == 1);
        batches.push((batch.len(), bytes));
    }
    batches
}

#[test]
fn preparation_batches_respect_record_and_byte_bounds() {
    assert_eq!(
        partitions(std::iter::repeat_n(1, 600)),
        vec![(256, 256), (256, 256), (88, 88)]
    );
    assert_eq!(
        partitions([4 * 1024 * 1024, 4 * 1024 * 1024, 1]),
        vec![(2, 8 * 1024 * 1024), (1, 1)]
    );
    assert_eq!(
        partitions([9 * 1024 * 1024, 1]),
        vec![(1, 9 * 1024 * 1024), (1, 1)]
    );
}
