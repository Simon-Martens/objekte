# Musenalm query checkpoint

Code revision: `4d2ac6623abcbed3237d7b56e6ce8f51e5c53a21`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 2685.580 | 3246.285 | 6323.680 | 6323.680 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4126.356 | 4139.400 | 4509.131 | 4509.131 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 1923.813 | 1978.968 | 2197.146 | 2197.146 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 19.091 | 19.468 | 25.590 | 25.590 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 17.815 | 18.539 | 24.906 | 24.906 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.656 | 16.666 | 20.289 | 20.289 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.812 s; parses 148486; resident 692 MiB; peak resident 732 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
