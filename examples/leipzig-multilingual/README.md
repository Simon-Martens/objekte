# Leipzig multilingual example

This example is a schema-free `texts` collection with 400,000 JSON documents:
50,000 news sentences each in Arabic (`ara`), German (`deu`), French (`fra`),
Japanese (`jpn`), Polish (`pol`), Russian (`rus`), Spanish (`spa`), and Chinese
(`zho`). Each file under `data/texts/` has one `text` field. Its filename combines
the language code and the sentence number from the source archive.

The sentences come from the Leipzig Corpora Collection's 2020 news archives.
See [the data attribution and license](DATA-LICENSE.md) before reusing them.
The original archives are not stored here. The [preparation script](../../crates/bench/prepare_leipzig_corpus.py)
can recreate the files; it verifies the archives against pinned SHA-256 hashes.

Run the [50-query workload](../../crates/bench/workloads/leipzig-multilingual-50.json)
against the example with either encoding:

```sh
cargo build --release -p objekte-bench --bin json-query-mix
target/release/json-query-mix --repo examples/leipzig-multilingual --workload crates/bench/workloads/leipzig-multilingual-50.json --no-schema --runs 7 --bigram-encoding ascii
target/release/json-query-mix --repo examples/leipzig-multilingual --workload crates/bench/workloads/leipzig-multilingual-50.json --no-schema --runs 7 --bigram-encoding utf8_bytes
```

The [benchmark report](../../crates/bench/BIGRAM-FOREIGN-50-2026-11-02.md)
contains the original timings. They were measured with these JSON files copied
to Linux `/tmp` (tmpfs), so timings from this checkout's storage may differ.
