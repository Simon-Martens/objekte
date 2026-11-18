# Musenalm query checkpoint

Code revision: `2e18871c0bcfb9cf9335d9e73525c64f91c960cf`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 2556.647 | 2792.484 | 6371.461 | 6371.461 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4187.378 | 4259.558 | 4380.558 | 4380.558 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 2010.415 | 2064.074 | 2274.067 | 2274.067 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 5.486 | 5.697 | 10.132 | 10.132 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 7.175 | 7.450 | 11.760 | 11.760 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 5.045 | 5.162 | 8.148 | 8.148 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.813 s; parses 148486; resident 661 MiB; peak resident 701 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
