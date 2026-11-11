# Musenalm query checkpoint

Code revision: `a1bb0990e86a8ed7a8c35c3e4a1610b5077c3bc3`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q022 | ascii-off | 8531.386 | 9989.405 | 16588.371 | 1493 | 1504 | 1504 | 20/21 | 0/21 | ok |
| Q110 | ascii-off | 14490.510 | 15865.423 | 24502.128 | 4911 | 4928 | 4928 | 20/21 | 0/21 | ok |
| Q190 | ascii-off | 19376.068 | 22741.380 | 32589.860 | 3821 | 3821 | 3821 | 20/21 | 0/21 | ok |
| Q022 | unicode-warm | 7.865 | 12.905 | 33.694 | 1493 | 0 | 0 | 21/21 | 21/21 | ok |
| Q110 | unicode-warm | 5.851 | 8.516 | 31.139 | 4911 | 0 | 0 | 21/21 | 21/21 | ok |
| Q190 | unicode-warm | 76332.462 | 80650.864 | 85324.263 | 3821 | 0 | 3821 | 21/21 | 21/21 | ok |

## Index construction

- "ascii": build 2.601 s; snapshot write 0.435 s; parses 148486; resident 466 MiB; peak resident 547 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.343/1.348/1.423 s over 5 runs.
- "unicode": build 2.648 s; snapshot write 0.439 s; parses 148486; resident 745 MiB; peak resident 745 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.339/1.356/1.407 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
