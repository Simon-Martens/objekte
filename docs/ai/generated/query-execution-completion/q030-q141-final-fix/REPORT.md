# Musenalm query checkpoint

Code revision: `67ec75be400be0127d55a40860767775636e3cf4`. Repetitions: 20. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 2623.380 | 2671.431 | 3745.394 | 5170.094 | 1264 | 2251 | 2251 | 0/20 | ok |
| Q141 | unicode-off | 4259.030 | 5075.654 | 7157.437 | 7192.680 | 823 | 982 | 982 | 0/20 | ok |

## Index construction

- "unicode": open/build 0.816 s; parses 148486; resident 674 MiB; peak resident 714 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
