# Musenalm query checkpoint

Code revision: `bf32a30e25001a026c26f4efbe73bfb058f007b3`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 3067.355 | 3374.481 | 5049.714 | 5049.714 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4419.635 | 4510.502 | 4677.610 | 4677.610 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 2037.047 | 2134.440 | 2397.178 | 2397.178 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 20.137 | 20.676 | 26.918 | 26.918 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 16.561 | 17.271 | 23.926 | 23.926 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 17.020 | 17.309 | 21.985 | 21.985 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.773 s; parses 148486; resident 667 MiB; peak resident 707 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
