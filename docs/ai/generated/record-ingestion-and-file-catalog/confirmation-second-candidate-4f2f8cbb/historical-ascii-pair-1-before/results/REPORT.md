# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q050 | ascii-off | 282.267 | 288.726 | 405.654 | 405.654 | 81 | 81 | 0 | 0/7 | ok |
| Q051 | ascii-off | 104.100 | 106.301 | 162.663 | 162.663 | 1942 | 1942 | 0 | 0/7 | ok |
| Q057 | ascii-off | 15.884 | 16.423 | 35.211 | 35.211 | 4177 | 4177 | 0 | 0/7 | ok |
| Q064 | ascii-off | 311.071 | 314.906 | 462.364 | 462.364 | 1 | 210 | 0 | 0/7 | ok |
| Q067 | ascii-off | 70.337 | 78.360 | 107.173 | 107.173 | 1 | 1 | 0 | 0/7 | ok |
| Q090 | ascii-off | 6.798 | 7.152 | 13.925 | 13.925 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.602 s; parses 148486; resident 565 MiB; peak resident 605 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
