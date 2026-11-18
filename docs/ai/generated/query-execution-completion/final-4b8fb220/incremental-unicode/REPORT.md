# Musenalm query checkpoint

Code revision: `4b8fb2207bef169eaa4d8ebf66262fbbc04fa35c`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 6476.843 | 8777.717 | 9342.524 | 9342.524 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 5555.091 | 7070.129 | 7740.475 | 7740.475 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 4225.254 | 4935.097 | 5240.719 | 5240.719 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 21.075 | 22.829 | 142.870 | 142.870 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 18.034 | 20.352 | 98.085 | 98.085 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.714 | 17.095 | 39.520 | 39.520 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.832 s; parses 148486; resident 663 MiB; peak resident 704 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
