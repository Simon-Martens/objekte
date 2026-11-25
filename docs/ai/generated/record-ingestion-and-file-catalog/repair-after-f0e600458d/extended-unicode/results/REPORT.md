# Musenalm query checkpoint

Code revision: `f0e600458d8804b57efb0a8a2b91478d1a39f667`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q155 | unicode-off | 147.029 | 149.952 | 244.035 | 244.035 | 3821 | 3821 | 0 | 0/7 | ok |
| Q163 | unicode-off | 5631.876 | 5823.231 | 7274.869 | 7274.869 | 1003 | 1627 | 1627 | 0/7 | ok |
| Q166 | unicode-off | 31354.269 | 32486.288 | 33847.880 | 33847.880 | 33619 | 0 | 0 | 0/7 | ok |
| Q170 | unicode-off | 46.560 | 51.042 | 139.700 | 139.700 | 1 | 1 | 2 | 0/7 | ok |
| Q195 | unicode-off | 28.128 | 28.575 | 41.171 | 41.171 | 0 | 0 | 0 | 0/7 | ok |
| Q219 | unicode-off | 158.754 | 163.876 | 191.084 | 191.084 | 3821 | 3821 | 0 | 0/7 | ok |

## Index construction

- "unicode": build 0.800 s; parses 148486; resident 662 MiB; peak resident 702 MiB; estimated index 103 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
