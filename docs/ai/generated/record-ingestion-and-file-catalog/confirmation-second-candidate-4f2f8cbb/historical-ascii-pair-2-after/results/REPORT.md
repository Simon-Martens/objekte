# Musenalm query checkpoint

Code revision: `4f2f8cbb3f88ca4e279509d9d2b60052312467c4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q050 | ascii-off | 279.441 | 284.034 | 417.017 | 417.017 | 81 | 81 | 0 | 0/7 | ok |
| Q051 | ascii-off | 107.658 | 108.319 | 159.939 | 159.939 | 1942 | 1942 | 0 | 0/7 | ok |
| Q057 | ascii-off | 17.698 | 18.292 | 36.589 | 36.589 | 4177 | 4177 | 0 | 0/7 | ok |
| Q064 | ascii-off | 302.833 | 307.750 | 594.655 | 594.655 | 1 | 210 | 0 | 0/7 | ok |
| Q067 | ascii-off | 69.038 | 74.908 | 109.238 | 109.238 | 1 | 1 | 0 | 0/7 | ok |
| Q090 | ascii-off | 6.818 | 7.088 | 14.382 | 14.382 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.612 s; parses 148486; resident 574 MiB; peak resident 614 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
