# Musenalm query checkpoint

Code revision: `f0df3ba68548a4e487556b3cbfcd7f0fba2db590`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 2887.931 | 3329.374 | 6075.393 | 6075.393 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4688.800 | 5121.783 | 5263.628 | 5263.628 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 2109.464 | 2138.549 | 2455.469 | 2455.469 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 19.153 | 19.804 | 25.736 | 25.736 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 18.240 | 19.231 | 24.866 | 24.866 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.641 | 17.696 | 20.726 | 20.726 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.797 s; parses 148486; resident 663 MiB; peak resident 704 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
