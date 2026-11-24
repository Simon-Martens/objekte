# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q039 | unicode-off | 23.709 | 24.016 | 81.367 | 81.367 | 0 | 0 | 0 | 0/7 | ok |
| Q050 | unicode-off | 279.895 | 282.553 | 348.705 | 348.705 | 81 | 81 | 0 | 0/7 | ok |
| Q051 | unicode-off | 102.625 | 105.149 | 149.299 | 149.299 | 1942 | 1942 | 0 | 0/7 | ok |
| Q056 | unicode-off | 93.811 | 94.322 | 127.941 | 127.941 | 11672 | 11672 | 0 | 0/7 | ok |
| Q061 | unicode-off | 10.660 | 11.520 | 67.592 | 67.592 | 1 | 1 | 1 | 0/7 | ok |
| Q091 | unicode-off | 23.656 | 24.939 | 47.819 | 47.819 | 3911 | 3911 | 0 | 0/7 | ok |
| Q108 | unicode-off | 38.358 | 41.702 | 62.171 | 62.171 | 2684 | 2684 | 0 | 0/7 | ok |
| Q109 | unicode-off | 80.679 | 84.575 | 107.862 | 107.862 | 2651 | 2651 | 0 | 0/7 | ok |
| Q110 | unicode-off | 22.190 | 22.776 | 47.771 | 47.771 | 4911 | 4911 | 0 | 0/7 | ok |
| Q124 | unicode-off | 160.588 | 228.578 | 240.870 | 240.870 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.689 s; parses 148486; resident 572 MiB; peak resident 612 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
