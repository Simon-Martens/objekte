# Musenalm query checkpoint

Code revision: `f0df3ba68548a4e487556b3cbfcd7f0fba2db590`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 2852.722 | 3781.807 | 6214.314 | 6214.314 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4483.161 | 4579.896 | 4803.914 | 4803.914 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 2374.142 | 2533.026 | 2801.013 | 2801.013 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 19.438 | 20.454 | 25.361 | 25.361 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 17.042 | 17.990 | 23.949 | 23.949 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 15.735 | 16.181 | 20.090 | 20.090 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.778 s; parses 148486; resident 659 MiB; peak resident 700 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
