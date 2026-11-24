# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 13.763 | 14.457 | 71.736 | 71.736 | 203 | 203 | 0 | 0/7 | ok |
| Q061 | ascii-off | 11.124 | 11.758 | 54.149 | 54.149 | 1 | 1 | 1 | 0/7 | ok |
| Q067 | ascii-off | 71.670 | 76.845 | 102.529 | 102.529 | 1 | 1 | 0 | 0/7 | ok |
| Q099 | ascii-off | 191.979 | 194.033 | 261.846 | 261.846 | 59586 | 59586 | 0 | 0/7 | ok |
| Q104 | ascii-off | 46.575 | 48.920 | 102.991 | 102.991 | 106 | 106 | 0 | 0/7 | ok |
| Q111 | ascii-off | 124.610 | 126.523 | 153.989 | 153.989 | 4928 | 4928 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.590 s; parses 148486; resident 533 MiB; peak resident 572 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
