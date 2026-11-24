# Musenalm query checkpoint

Code revision: `55f6bb8ba596f1503d232f86a67a58a233c441b4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 37271.765 | 38333.608 | 42074.193 | 42074.193 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8353.546 | 8533.964 | 9695.000 | 9695.000 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.878 | 4.003 | 32.554 | 32.554 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 21.238 | 22.190 | 46.599 | 46.599 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1518.100 | 1546.599 | 1711.595 | 1711.595 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3246.523 | 3266.887 | 3536.492 | 3536.492 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 187.358 | 190.017 | 251.453 | 251.453 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 3080.686 | 3326.256 | 5254.319 | 5254.319 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 63550.496 | 64215.793 | 65098.088 | 65098.088 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.759 s; parses 148486; resident 607 MiB; peak resident 648 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
