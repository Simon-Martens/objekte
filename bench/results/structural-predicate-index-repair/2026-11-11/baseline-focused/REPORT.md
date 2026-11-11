# Musenalm query checkpoint

Code revision: `95f1736d448a01d94650afd237149c158c0e7a8a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q115 | unicode-off | 18192.013 | 19395.315 | 27478.387 | 244 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q116 | unicode-off | 442729.759 | 472949.430 | 533772.110 | 70428 | 133948 | 133948 | 6/7 | 0/7 | ok |
| Q117 | unicode-off | 404834.463 | 429785.120 | 435558.691 | 63520 | 133948 | 133948 | 6/7 | 0/7 | ok |
| Q118 | unicode-off | 397678.953 | 402704.309 | 412251.404 | 254 | 133948 | 133948 | 6/7 | 0/7 | ok |
| Q120 | unicode-off | 369942.291 | 403197.747 | 404715.790 | 42700 | 133948 | 133948 | 6/7 | 0/7 | ok |

## Index construction

- "unicode": build 3.076 s; snapshot write 0.441 s; parses 148486; resident 540 MiB; peak resident 566 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.353/1.375/1.432 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
