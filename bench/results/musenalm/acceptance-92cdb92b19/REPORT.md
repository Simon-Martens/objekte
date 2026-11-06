# Optimized versus index-free acceptance

Measured code: `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`.

`cargo build --release --example accept_musenalm` followed by
`target/release/examples/accept_musenalm --alphabet ascii` and the same command
with `--alphabet unicode` passed all 150 cases under each alphabet. The repository
was restored from a snapshot in both runs. The raw case-by-case logs are
`ascii.log` and `unicode.log` in this directory.

For each case, the acceptance program compares optimized execution with the
index-free reference evaluator: count, ordered page length, collection and
record IDs, generation, file ID/path/size/timestamps, original byte range,
matched-field evidence, and requested payload. The program does not compare
the new status/classification/diagnostic fields; focused tests cover those
fields across build, query, cache and restore routes. Complete membership is
also checked independently against jq in `../jq-oracle-92cdb92b19/`.
