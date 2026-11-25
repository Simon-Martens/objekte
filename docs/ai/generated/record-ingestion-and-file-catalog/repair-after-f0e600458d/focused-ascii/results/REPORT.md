# Musenalm query checkpoint

Code revision: `f0e600458d8804b57efb0a8a2b91478d1a39f667`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 36501.250 | 37880.865 | 41325.071 | 41325.071 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8281.875 | 8287.034 | 9691.547 | 9691.547 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.696 | 3.985 | 30.252 | 30.252 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 20.793 | 22.130 | 43.972 | 43.972 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1517.239 | 1529.854 | 1709.976 | 1709.976 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3228.467 | 3251.345 | 3517.619 | 3517.619 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 193.191 | 195.521 | 242.991 | 242.991 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 2617.956 | 2835.602 | 4804.528 | 4804.528 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 62498.507 | 63674.965 | 66405.210 | 66405.210 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.651 s; parses 148486; resident 595 MiB; peak resident 634 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
