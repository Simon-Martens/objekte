# Musenalm query checkpoint

Code revision: `4de52f87d20a574a902e3becf454d3ea57a68eac`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 36422.982 | 37070.490 | 40385.585 | 40385.585 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8219.883 | 8270.261 | 9769.344 | 9769.344 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.590 | 3.846 | 28.427 | 28.427 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 20.573 | 21.843 | 45.149 | 45.149 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1514.908 | 1524.532 | 1692.165 | 1692.165 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3258.244 | 3272.957 | 3527.736 | 3527.736 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 193.726 | 194.798 | 244.247 | 244.247 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 2669.721 | 2774.400 | 4731.228 | 4731.228 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 62854.685 | 63903.442 | 65539.717 | 65539.717 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.749 s; parses 148486; resident 599 MiB; peak resident 639 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
