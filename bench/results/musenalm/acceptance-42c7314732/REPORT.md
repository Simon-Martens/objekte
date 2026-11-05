# Independent scanner acceptance

Code revision: `42c7314732`. Both runs used the fixed 150-case manifest at
`core/benches/musenalm_queries.json` and the repository fixture at
`examples/musenalm`. The optimized engine and index-free scanner agreed on
COUNT, page order, record/file identity, byte ranges, recorded revision,
matched-field evidence, and requested payload for every case under both ASCII
and Unicode bigrams: **300/300 comparisons passed**.

The runs rebuilt snapshots because the model version was raised for exact large
JSON integers. Each log has one line per case and a final summary. This is a
correctness check, not a timing sample.

Reproduce:

```bash
cargo run --release --example accept_musenalm -- --alphabet ascii
cargo run --release --example accept_musenalm -- --alphabet unicode
```
