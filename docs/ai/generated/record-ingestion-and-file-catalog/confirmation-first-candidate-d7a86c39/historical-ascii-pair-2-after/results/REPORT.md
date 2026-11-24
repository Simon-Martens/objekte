# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 14.455 | 15.155 | 71.416 | 71.416 | 203 | 203 | 0 | 0/7 | ok |
| Q061 | ascii-off | 10.495 | 11.328 | 56.065 | 56.065 | 1 | 1 | 1 | 0/7 | ok |
| Q067 | ascii-off | 75.253 | 75.985 | 113.734 | 113.734 | 1 | 1 | 0 | 0/7 | ok |
| Q099 | ascii-off | 192.330 | 194.589 | 268.504 | 268.504 | 59586 | 59586 | 0 | 0/7 | ok |
| Q104 | ascii-off | 47.538 | 49.000 | 106.646 | 106.646 | 106 | 106 | 0 | 0/7 | ok |
| Q111 | ascii-off | 150.831 | 152.366 | 179.503 | 179.503 | 4928 | 4928 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.703 s; parses 148486; resident 577 MiB; peak resident 617 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
