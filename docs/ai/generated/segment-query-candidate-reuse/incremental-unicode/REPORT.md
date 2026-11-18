# Musenalm query checkpoint

Code revision: `98b5d3a23af28071cdd7f5ad01186e46f8c21702`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 3584.871 | 3723.823 | 5962.937 | 5962.937 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4480.058 | 4627.208 | 5277.191 | 5277.191 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 2126.549 | 2233.219 | 2520.719 | 2520.719 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 18.428 | 18.881 | 24.430 | 24.430 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 16.749 | 16.892 | 23.981 | 23.981 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 15.043 | 15.454 | 18.902 | 18.902 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.777 s; parses 148486; resident 651 MiB; peak resident 691 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
