use super::*;

#[test]
fn buffered_fingerprints_preserve_existing_bytes_across_buffer_boundaries() {
    let records = (0..2000)
        .map(|i| {
            (
                format!("record-{i}"),
                serde_json::json!({
                    "size":i,"modified":"-123456789","identity":"device:inode:ctime",
                    "escaped":"quote\" slash\\ newline\n é 🐈", "large":"x".repeat(i%90)
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for domain in ["source-manifest/v2", "different-domain"] {
        assert_eq!(
            fingerprint(domain, &records),
            digest(&serde_json::to_vec(&(domain, &records)).unwrap())
        );
    }
    assert_eq!(
        fingerprint("empty", &Vec::<String>::new()),
        digest(br#"["empty",[]]"#)
    );
    let oversized = "a".repeat(128 * 1024);
    assert_eq!(
        fingerprint("large", &oversized),
        digest(&serde_json::to_vec(&("large", &oversized)).unwrap())
    );
}
