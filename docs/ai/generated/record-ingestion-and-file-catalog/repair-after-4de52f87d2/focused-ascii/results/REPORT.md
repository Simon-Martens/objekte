# Musenalm query checkpoint

Code revision: `4de52f87d20a574a902e3becf454d3ea57a68eac`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 36417.731 | 38508.620 | 40934.373 | 40934.373 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | ascii-off | 8224.849 | 8280.894 | 9663.683 | 9663.683 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | ascii-off | 3.618 | 3.844 | 27.662 | 27.662 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | ascii-off | 21.684 | 30.745 | 50.180 | 50.180 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | ascii-off | 1524.548 | 1535.491 | 1689.248 | 1689.248 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | ascii-off | 3243.286 | 3255.927 | 3536.418 | 3536.418 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | ascii-off | 193.610 | 195.716 | 241.892 | 241.892 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | ascii-off | 2632.522 | 2826.936 | 4701.256 | 4701.256 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | ascii-off | 63491.012 | 64883.320 | 65256.210 | 65256.210 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "ascii": build 0.604 s; parses 148486; resident 581 MiB; peak resident 621 MiB; estimated index 99 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
