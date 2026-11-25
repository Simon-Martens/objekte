# Musenalm query checkpoint

Code revision: `34d17b7b82c8398fa906f3c64e611e36fec7c521`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 36050.398 | 37364.203 | 40140.683 | 40140.683 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8328.265 | 8874.157 | 9744.827 | 9744.827 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.676 | 3.910 | 26.935 | 26.935 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 21.226 | 23.481 | 43.939 | 43.939 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1545.264 | 1553.913 | 1709.362 | 1709.362 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3245.374 | 3263.636 | 3524.579 | 3524.579 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 193.541 | 195.551 | 241.884 | 241.884 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 2672.992 | 2864.244 | 4897.927 | 4897.927 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 62596.445 | 64353.296 | 65765.610 | 65765.610 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.654 s; parses 148486; resident 588 MiB; peak resident 628 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
