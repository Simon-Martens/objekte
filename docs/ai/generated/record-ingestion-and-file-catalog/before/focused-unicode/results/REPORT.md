# Musenalm query checkpoint

Code revision: `d8d703dbd24e1ef72f9a37cf7a235a7a559e0598`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 36811.788 | 37329.729 | 41060.326 | 41060.326 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8348.735 | 8754.809 | 9720.307 | 9720.307 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.758 | 3.877 | 25.533 | 25.533 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 20.507 | 21.790 | 41.958 | 41.958 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1509.735 | 1514.742 | 1670.515 | 1670.515 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3261.619 | 3310.653 | 3722.447 | 3722.447 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 185.585 | 187.594 | 240.508 | 240.508 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 2973.160 | 3206.849 | 5338.513 | 5338.513 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 61812.348 | 63750.968 | 65654.400 | 65654.400 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.649 s; parses 148486; resident 563 MiB; peak resident 603 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
