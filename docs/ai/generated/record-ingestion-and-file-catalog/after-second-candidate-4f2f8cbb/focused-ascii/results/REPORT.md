# Musenalm query checkpoint

Code revision: `4f2f8cbb3f88ca4e279509d9d2b60052312467c4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 36704.160 | 37962.937 | 40924.144 | 40924.144 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8495.736 | 8649.053 | 9799.334 | 9799.334 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.710 | 3.874 | 26.892 | 26.892 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 20.446 | 22.114 | 45.698 | 45.698 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1517.367 | 1523.264 | 1672.402 | 1672.402 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3244.656 | 3271.382 | 3668.051 | 3668.051 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 188.603 | 192.580 | 239.798 | 239.798 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 2994.464 | 3141.456 | 5143.362 | 5143.362 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 62832.755 | 64238.528 | 66096.210 | 66096.210 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.710 s; parses 148486; resident 595 MiB; peak resident 635 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
