# Scanner checkpoint

Baseline: integration `8ca2851ab8` binary SHA-256 `9baebc3cbc615fd4528cbea7b9716ddbfcdf94ce2dc069ad8c85eb990695a358`.
Candidate: source based on `a093684647` plus the scanner changes in this commit, binary SHA-256 `c8ce6d323e9b3b5de8f6c59eb0a140b96263b9ca2a7cdb8e2ecfa688b2eaa6e6`. Both binaries used the same full Musenalm fixture at `/home/simon/source/objekte-integration-hybrid-index-storage/examples/musenalm` and the command shape below. The candidate binary predates the subsequent posting-cache change.

`bench_musenalm --root .../examples/musenalm --extended --incremental --profiles ascii-off --cases Q006,Q007,Q025,Q026 --repetitions 10 --verified-cache-bytes 0 --prepared-cache-bytes 16777216 --query-workers 32 --result-dir <mode>`

| Case | Baseline median ms | Candidate median ms | Ratio |
| --- | ---: | ---: | ---: |
| Q006 | 129.836 | 122.953 | 0.947 |
| Q007 | 167.702 | 152.526 | 0.910 |
| Q025 | 68.889 | 60.959 | 0.885 |
| Q026 | 71.808 | 64.383 | 0.897 |

All four result digests and scanned-record counts match. These are sequential fresh-process checkpoints, not interleaved paired evidence; the final campaign must recheck the ratios across profiles. Q006 improves about 5.3%, below the 10% target. The change defers evidence only for eligible borrowed source scans; opaque/custom filter paths keep their original single evaluation.
