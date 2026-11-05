# Amazon complete-result timing: OBJEKTE and jq

Measured `master` at `94e1bd82abca6d39d2e480018c31c75bd0b8c3f7` on 2026-11-05. The same million-review fixture, `headphones` filters, and complete ID output from the [result parity report](REPORT.md) were used. Every one of the 60 measured outputs matched its expected complete-result SHA-256. Raw chronological samples and exact commands are in [timings.ndjson](timings.ndjson); the runners are [Rust](../../examples/amazon_jq_timing.rs), [jq](jq_timing.py), and [orchestration](run_timing.py).

Three interleaved samples per case; entries are median milliseconds with min–max in parentheses. `First` means a fresh process/repository handle with no prior query in that handle. The existing on-disk OBJEKTE index was allowed to restore; this is **not cold physical storage**. `Warm` means one complete untimed query preceded the measured query in the same OBJEKTE handle. jq repeated its archive/file pass after an untimed pass; it has no retained query index. OBJEKTE's verified-result cache was disabled. All processes were pinned to CPUs 0–15; OBJEKTE used 16 Rayon threads, while jq used its normal single-threaded execution.

| Query | Route | First (ms) | Warm (ms) |
| --- | --- | ---: | ---: |
| `text ~ 'headphones'` | OBJEKTE indexed | 2,374 (2,326–2,474) | 260 (166–277) |
| | jq, archive via Python gzip feeder | 4,672 (4,663–4,672) | 4,681 (4,681–4,717) |
| | jq, direct `gzip → jq` | 5,065 (5,057–5,068) | 5,063 (5,062–5,079) |
| | jq, million individual files | 8,685 (8,630–8,712) | 8,564 (8,549–8,712) |
| | OBJEKTE direct scan | 8,499 (8,498–8,499) | 8,291 (8,254–8,323) |
| `text !~ 'headphones'` | OBJEKTE indexed | 10,818 (10,556–10,844) | 7,634 (7,618–7,973) |
| | jq, archive via Python gzip feeder | 5,616 (5,599–5,618) | 5,623 (5,611–5,660) |
| | jq, direct `gzip → jq` | 6,026 (6,003–6,137) | 6,025 (6,007–6,031) |
| | jq, million individual files | 9,221 (9,027–9,225) | 9,212 (8,912–9,229) |
| | OBJEKTE direct scan | 9,025 (8,984–9,057) | 8,678 (8,677–8,714) |

Against the faster jq archive feeder, indexed OBJEKTE is about **2.0× faster on the first positive query** and **18.0× faster after query warmup**. For the broad negated query it is **1.9× slower first** and **1.36× slower warm**. The negative predicate still scans one million candidates and returns 986,879 records; the positive predicate scans 16,874 candidates and returns 13,121. The per-file jq route is a closer storage-layout control, but its Python file feeder adds overhead. Archive jq benefits from one compressed sequential stream, whereas OBJEKTE opens individual source files. The numbers compare complete-query pipelines, not isolated matcher implementations or equal CPU usage.

The Rust timing driver reports query duration including full result streaming and output; its repository open cost was around 0.17 s per process and is **not included** in the table. The jq durations include decompression, parsing and full output, but not the small Python controller startup. Filesystem caches were already warm; no OS cache eviction was performed. The first OBJEKTE indexed query includes index setup or restoration in that repository handle. Timings are specific to this corpus, query, hardware and storage layout.
