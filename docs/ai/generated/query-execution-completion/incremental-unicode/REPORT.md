# Musenalm query checkpoint

Code revision: `67ec75be400be0127d55a40860767775636e3cf4`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 6194.596 | 7306.829 | 9108.363 | 9108.363 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 6483.647 | 7165.709 | 7457.684 | 7457.684 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 3192.900 | 3462.658 | 4224.283 | 4224.283 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 19.410 | 20.306 | 74.621 | 74.621 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 17.723 | 18.532 | 91.241 | 91.241 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.410 | 16.846 | 35.528 | 35.528 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.897 s; parses 148486; resident 651 MiB; peak resident 692 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
