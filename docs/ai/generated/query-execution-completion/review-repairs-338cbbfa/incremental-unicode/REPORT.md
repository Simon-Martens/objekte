# Musenalm query checkpoint

Code revision: `338cbbfae7c1878ac12cd7af3d0c0b27015dbc84`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 4203.511 | 4704.765 | 5695.379 | 5695.379 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4849.865 | 5154.415 | 5786.787 | 5786.787 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 2054.443 | 2246.760 | 3013.577 | 3013.577 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 18.587 | 23.515 | 46.088 | 46.088 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 17.308 | 21.782 | 70.688 | 70.688 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.143 | 16.959 | 77.087 | 77.087 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.935 s; parses 148486; resident 680 MiB; peak resident 720 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
