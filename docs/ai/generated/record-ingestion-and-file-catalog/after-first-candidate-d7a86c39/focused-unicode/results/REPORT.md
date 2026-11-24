# Musenalm query checkpoint

Code revision: `d7a86c3999c8947a38b36465318b2ca781b6dcc8`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 39524.334 | 39946.763 | 41791.153 | 41791.153 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8823.799 | 8910.083 | 10291.283 | 10291.283 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.841 | 3.998 | 31.045 | 31.045 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 21.565 | 22.844 | 45.447 | 45.447 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1602.257 | 1612.448 | 1773.442 | 1773.442 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3478.469 | 3491.525 | 3839.901 | 3839.901 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 203.907 | 206.716 | 265.078 | 265.078 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 3071.919 | 3100.755 | 5295.857 | 5295.857 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 65928.490 | 69884.069 | 70584.581 | 70584.581 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.716 s; parses 148486; resident 590 MiB; peak resident 630 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
