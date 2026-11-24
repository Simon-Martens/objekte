# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q050 | ascii-off | 271.998 | 276.953 | 383.589 | 383.589 | 81 | 81 | 0 | 0/7 | ok |
| Q051 | ascii-off | 103.001 | 105.234 | 151.659 | 151.659 | 1942 | 1942 | 0 | 0/7 | ok |
| Q057 | ascii-off | 16.503 | 17.030 | 34.275 | 34.275 | 4177 | 4177 | 0 | 0/7 | ok |
| Q064 | ascii-off | 295.505 | 300.313 | 461.255 | 461.255 | 1 | 210 | 0 | 0/7 | ok |
| Q067 | ascii-off | 69.598 | 77.646 | 108.615 | 108.615 | 1 | 1 | 0 | 0/7 | ok |
| Q090 | ascii-off | 6.716 | 7.275 | 13.134 | 13.134 | 0 | 0 | 0 | 0/7 | ok |

## Index construction

- "ascii": build 0.610 s; parses 148486; resident 554 MiB; peak resident 594 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
