# Musenalm query checkpoint

Code revision: `4b8fb2207bef169eaa4d8ebf66262fbbc04fa35c`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | ascii-off | 2601.769 | 2633.380 | 4360.786 | 6285.620 | 1264 | 2251 | 2251 | 0/20 | ok |
| Q164 | ascii-off | 5882.190 | 7577.445 | 11478.522 | 14345.659 | 4928 | 4928 | 4928 | 0/20 | ok |

## Index construction

- "ascii": build 0.761 s; parses 148486; resident 649 MiB; peak resident 692 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
