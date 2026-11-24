# Musenalm query checkpoint

Code revision: `4f2f8cbb3f88ca4e279509d9d2b60052312467c4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q093 | unicode-off | 25.938 | 26.808 | 98.092 | 98.092 | 3856 | 3856 | 0 | 0/7 | ok |
| Q097 | unicode-off | 55.868 | 58.818 | 81.335 | 81.335 | 0 | 0 | 0 | 0/7 | ok |
| Q107 | unicode-off | 85.889 | 91.645 | 127.693 | 127.693 | 2684 | 2684 | 0 | 0/7 | ok |
| Q115 | unicode-off | 23.031 | 23.484 | 49.321 | 49.321 | 244 | 244 | 0 | 0/7 | ok |
| Q121 | unicode-off | 28.664 | 30.258 | 71.675 | 71.675 | 341 | 341 | 0 | 0/7 | ok |
| Q137 | unicode-off | 172.249 | 179.461 | 340.906 | 340.906 | 81 | 81 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.696 s; parses 148486; resident 581 MiB; peak resident 621 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
