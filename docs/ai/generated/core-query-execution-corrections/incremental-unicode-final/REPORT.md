# Musenalm query checkpoint

Code revision: `4d2ac6623abcbed3237d7b56e6ce8f51e5c53a21`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 2741.152 | 3437.136 | 5166.780 | 5166.780 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4190.114 | 4293.417 | 4437.841 | 4437.841 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 1925.335 | 2000.281 | 2284.403 | 2284.403 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 18.853 | 19.062 | 25.841 | 25.841 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 17.368 | 18.479 | 23.561 | 23.561 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.390 | 16.706 | 21.238 | 21.238 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.769 s; parses 148486; resident 653 MiB; peak resident 693 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
