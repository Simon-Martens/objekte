# Musenalm query checkpoint

Code revision: `55f6bb8ba596f1503d232f86a67a58a233c441b4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 36159.765 | 39001.834 | 40830.271 | 40830.271 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8393.631 | 8603.575 | 9796.571 | 9796.571 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.902 | 4.163 | 31.178 | 31.178 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 21.121 | 21.876 | 45.463 | 45.463 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1510.213 | 1521.970 | 1669.930 | 1669.930 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3279.481 | 3291.210 | 3576.077 | 3576.077 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 188.492 | 193.322 | 253.133 | 253.133 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 3031.500 | 3139.120 | 5288.881 | 5288.881 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 64348.914 | 65680.732 | 66107.154 | 66107.154 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.647 s; parses 148486; resident 584 MiB; peak resident 624 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
