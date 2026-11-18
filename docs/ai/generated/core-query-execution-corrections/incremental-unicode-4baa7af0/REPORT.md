# Musenalm query checkpoint

Code revision: `4baa7af067f74f5ffb5772ef641ff0994fc4173f`. Repetitions: 5. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q030 | unicode-off | 2565.240 | 2893.280 | 6197.500 | 6197.500 | 1264 | 2251 | 2251 | 0/5 | ok |
| Q141 | unicode-off | 4187.134 | 4297.887 | 4383.196 | 4383.196 | 823 | 982 | 982 | 0/5 | ok |
| Q163 | unicode-off | 1922.536 | 1958.220 | 2198.652 | 2198.652 | 1003 | 1627 | 1627 | 0/5 | ok |
| Q030 | unicode-warm | 18.247 | 18.823 | 26.241 | 26.241 | 1264 | 0 | 0 | 5/5 | ok |
| Q141 | unicode-warm | 17.585 | 17.942 | 25.360 | 25.360 | 823 | 0 | 0 | 5/5 | ok |
| Q163 | unicode-warm | 16.106 | 16.245 | 19.973 | 19.973 | 1003 | 0 | 0 | 5/5 | ok |

## Index construction

- "unicode": open/build 0.784 s; parses 148486; resident 653 MiB; peak resident 693 MiB; estimated index 110 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
