# Amazon review result parity with jq

## Question and inputs

This checks **complete result IDs**, not query speed. The repository was `master` at `6f6bedbb1c57aadbf0a19d184acb593d734d9b7e`. The fixture has 1,000,000 individual JSON review files in `examples/amazon-reviews-2023/data/reviews`, imported in order from the first 1,000,000 rows of `Musical_Instruments.jsonl.gz`. The archive SHA-256 is `0e7bf61672d2a984b153d6999ac7321fb4b29642f05c04496f54507437bbbfee`. Its import manifest matches the archive, and the current million-file corpus matches the previously recorded fingerprint. See [input-manifest.json](input-manifest.json).

The two queries are `text ~ 'headphones'` and `text !~ 'headphones'`. Here `~` is a case-sensitive literal substring match. The jq oracle tests `(.text | contains("headphones"))`; the negated case tests its complement for string `text` values. It also reports nonstring values separately. There were none. This is a deliberately narrow comparison of these predicates, not an oracle for other query modes or fields.

The independent evaluator was jq 1.8.1, parsing the original JSONL archive. Python numbered the rows and split jq's classifications into ID lists; it did not parse the JSON or evaluate the predicate. A one-million-row file import gives row N the ID `N` padded to seven digits. OBJEKTE 0.1.0 used Rust/Cargo 1.99.1 and the release [public-API driver](../../examples/amazon_result_parity.rs), executable SHA-256 `65ac90c0ad8df188076548b55dd42f763aef6c3677485fd664a4823cf40894e9`. The Cargo.lock SHA-256 was `2e219967e2327f963a8a7863bc649fdcccaf52b555de336604a810d72f003e52`. No production code changed for this comparison.

## Commands

Run from the repository root:

```sh
cargo build --locked --release -p objekte-bench --example amazon_result_parity -j 2
python3 crates/bench/results/amazon-external-result-parity-20261105/oracle_jq.py

mkdir -p target/benchmarks/amazon-external-result-parity-20261105
RAYON_NUM_THREADS=16 target/release/examples/amazon_result_parity examples/amazon-reviews-2023 bigram "text ~ 'headphones'" target/benchmarks/amazon-external-result-parity-20261105/objekte-bigram-positive.ids > target/benchmarks/amazon-external-result-parity-20261105/objekte-bigram-positive.json
RAYON_NUM_THREADS=16 target/release/examples/amazon_result_parity examples/amazon-reviews-2023 direct "text ~ 'headphones'" target/benchmarks/amazon-external-result-parity-20261105/objekte-direct-positive.ids > target/benchmarks/amazon-external-result-parity-20261105/objekte-direct-positive.json
RAYON_NUM_THREADS=16 target/release/examples/amazon_result_parity examples/amazon-reviews-2023 bigram "text !~ 'headphones'" target/benchmarks/amazon-external-result-parity-20261105/objekte-bigram-negative.ids > target/benchmarks/amazon-external-result-parity-20261105/objekte-bigram-negative.json
RAYON_NUM_THREADS=16 target/release/examples/amazon_result_parity examples/amazon-reviews-2023 direct "text !~ 'headphones'" target/benchmarks/amazon-external-result-parity-20261105/objekte-direct-negative.ids > target/benchmarks/amazon-external-result-parity-20261105/objekte-direct-negative.json

python3 crates/bench/results/amazon-external-result-parity-20261105/compare_results.py
```

The driver sets `query_cache_bytes = 0`, warms the ordinary bigram index, streams every result, asks for an uncapped count (`total_limit = 1,000,001`), and rejects truncation or a count different from the number returned. Thus a verified-result cache cannot hide candidate errors. Raw full ID lists, jq classifications, and OBJEKTE work summaries remain locally under ignored `target/benchmarks/amazon-external-result-parity-20261105/` to avoid committing roughly 25 MB of generated output. The [comparison summary](result-parity.json) contains all counts, work counters, mismatch counts and canonical ID hashes.

## Full-result comparison

| Query | jq IDs | OBJEKTE indexed IDs | OBJEKTE direct IDs | Missing IDs | Extra IDs | Sorted ID SHA-256 |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| `text ~ 'headphones'` | 13,121 | 13,121 | 13,121 | 0 | 0 | `00f0a33918e4e812f42910f438e8f1af6e046e4bc0cc8d79c2001c1356af3004` |
| `text !~ 'headphones'` | 986,879 | 986,879 | 986,879 | 0 | 0 | `c12a3910ff954489948fd487a0602eaa09b4cc9fff2ca1a454f4dd97cbc991cd` |

The positive and negative jq sets are disjoint and their union contains all 1,000,000 source IDs. Each OBJEKTE list has unique IDs, the same complete set and the same uncapped total as jq. Both OBJEKTE backends returned the same ordering: exactly reverse archive order, consistent with the repository's default order. No returned list was truncated; both result-cache hit counters were zero.

The indexed positive query performed 16,874 candidate scans and 27,161 source reads/parses, versus 1,000,000 reads/parses for direct evaluation. The indexed negative query performed 1,000,000 candidate scans and 1,986,879 source reads/parses; the negative condition cannot safely use the positive bigram posting as a complete complement, and the large returned result also requires materialization. These are **work counters**, not paired latency measurements. They explain why a selective positive query can do much less source work without losing matches, while the negated query can behave very differently.

## Checks and limits

The release example built successfully; `cargo clippy --locked -p objekte-bench --example amazon_result_parity -- -D warnings` and Python compilation passed. The repository-wide spacing check failed on already existing unrelated Rust formatting differences in core and extension-test files; no production files were reformatted for this benchmark.

This comparison does not establish correctness for arbitrary filters, Unicode case folding, markup, metadata, or TUI request settings. It does establish full ID parity for these two Amazon predicates across the independent jq evaluator, direct OBJEKTE scanning, and indexed OBJEKTE execution on the stated fixture.
