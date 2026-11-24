# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q031 | ascii-off | 14.258 | 15.548 | 77.913 | 77.913 | 203 | 203 | 0 | 0/7 | ok |
| Q061 | ascii-off | 10.627 | 11.554 | 55.127 | 55.127 | 1 | 1 | 1 | 0/7 | ok |
| Q067 | ascii-off | 74.289 | 75.979 | 112.484 | 112.484 | 1 | 1 | 0 | 0/7 | ok |
| Q099 | ascii-off | 192.842 | 195.625 | 262.543 | 262.543 | 59586 | 59586 | 0 | 0/7 | ok |
| Q104 | ascii-off | 47.792 | 49.465 | 102.792 | 102.792 | 106 | 106 | 0 | 0/7 | ok |
| Q111 | ascii-off | 151.723 | 153.214 | 180.457 | 180.457 | 4928 | 4928 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.656 s; parses 148486; resident 581 MiB; peak resident 621 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
