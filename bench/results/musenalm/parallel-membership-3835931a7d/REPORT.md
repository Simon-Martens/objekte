# File-membership construction attribution

The same release benchmark harness ran against the serial builder at `23074150d4` and the bitmap builder at `3835931a7d` in old–new–new–old process order. Ten timed samples per file count were retained. Each file has one record. Timings include `include_file` and `add_record`, including identity insertion, and exclude inventory creation and final validation. No source parsing or bigram construction is involved. Machine: Rust 1.99.1, performance governor, affinity CPUs 0–31.

| Files | Old median ms | New median ms | Difference ms | Old range ms | New range ms |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 0.461 | 0.137 | -0.324 | 0.293–0.663 | 0.086–0.239 |
| 2,000 | 0.956 | 0.253 | -0.703 | 0.933–1.628 | 0.162–0.485 |
| 4,000 | 3.414 | 0.365 | -3.049 | 3.296–3.715 | 0.328–0.605 |
| 8,000 | 12.621 | 0.670 | -11.950 | 12.428–12.946 | 0.666–1.205 |
| 16,000 | 48.730 | 1.429 | -47.301 | 48.340–52.687 | 1.397–2.406 |

This isolates the removed growing-vector membership searches. The full Musenalm build effect is measured later with bigram construction and parallel scheduling separated.
