# Musenalm query checkpoint

Code revision: `34d17b7b82c8398fa906f3c64e611e36fec7c521`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 36415.824 | 37100.151 | 42242.536 | 42242.536 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8241.020 | 8367.658 | 9660.007 | 9660.007 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.746 | 3.925 | 27.270 | 27.270 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 21.141 | 21.661 | 45.611 | 45.611 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1520.807 | 1547.634 | 1697.751 | 1697.751 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3230.664 | 3241.097 | 3507.355 | 3507.355 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 194.157 | 195.837 | 243.098 | 243.098 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 2627.265 | 2810.522 | 4693.003 | 4693.003 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 62628.756 | 63767.723 | 67005.649 | 67005.649 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.691 s; parses 148486; resident 588 MiB; peak resident 628 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
