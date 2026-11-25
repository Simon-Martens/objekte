# Musenalm query checkpoint

Code revision: `7ce3f6e525fee77f46c71ad7c02a94c7d06a21b7`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 35137.852 | 35983.230 | 40892.020 | 40892.020 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8108.469 | 8155.685 | 9535.414 | 9535.414 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.761 | 3.923 | 26.459 | 26.459 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 21.611 | 22.750 | 44.001 | 44.001 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1533.529 | 1536.506 | 1695.905 | 1695.905 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3166.861 | 3188.968 | 3431.252 | 3431.252 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 195.169 | 196.177 | 243.467 | 243.467 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 2685.988 | 2857.956 | 4776.740 | 4776.740 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 62427.791 | 63311.990 | 67348.101 | 67348.101 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.696 s; parses 148486; resident 599 MiB; peak resident 639 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
