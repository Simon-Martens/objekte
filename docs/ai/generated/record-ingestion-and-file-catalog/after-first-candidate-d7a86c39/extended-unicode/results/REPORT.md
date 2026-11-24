# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 148.715 | 150.804 | 266.487 | 266.487 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5975.514 | 6128.853 | 7346.178 | 7346.178 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 32077.409 | 32692.724 | 34627.110 | 34627.110 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 44.458 | 48.803 | 152.481 | 152.481 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 27.958 | 28.315 | 40.993 | 40.993 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 159.289 | 161.053 | 216.401 | 216.401 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.852 s; parses 148486; resident 702 MiB; peak resident 744 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
