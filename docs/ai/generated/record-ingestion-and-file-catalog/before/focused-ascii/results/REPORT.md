# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 36993.728 | 37796.561 | 42830.223 | 42830.223 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8382.436 | 8591.492 | 9588.798 | 9588.798 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.666 | 3.974 | 26.464 | 26.464 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 20.415 | 21.383 | 43.877 | 43.877 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1512.586 | 1513.192 | 1676.272 | 1676.272 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3233.827 | 3254.556 | 3655.012 | 3655.012 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 185.935 | 188.966 | 237.797 | 237.797 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 2985.339 | 3206.729 | 5308.040 | 5308.040 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 61975.366 | 63590.812 | 64119.223 | 64119.223 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.594 s; parses 148486; resident 540 MiB; peak resident 581 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
