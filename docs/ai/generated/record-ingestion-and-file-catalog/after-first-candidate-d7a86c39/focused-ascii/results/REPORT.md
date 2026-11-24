# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 36960.679 | 39580.080 | 41732.691 | 41732.691 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8245.217 | 8412.768 | 10196.044 | 10196.044 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.745 | 3.871 | 54.310 | 54.310 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 21.079 | 21.970 | 43.556 | 43.556 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1509.843 | 1517.372 | 1701.818 | 1701.818 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3252.338 | 3270.258 | 3551.340 | 3551.340 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 194.081 | 196.147 | 245.628 | 245.628 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 2990.301 | 3102.997 | 5072.114 | 5072.114 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 63798.790 | 65462.961 | 67516.729 | 67516.729 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.633 s; parses 148486; resident 590 MiB; peak resident 629 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
