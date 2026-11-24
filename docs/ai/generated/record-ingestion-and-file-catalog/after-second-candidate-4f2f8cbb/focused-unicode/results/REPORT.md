# Musenalm query checkpoint

Code revision: `4f2f8cbb3f88ca4e279509d9d2b60052312467c4`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | unicode-off | 36819.757 | 37673.154 | 41808.295 | 41808.295 | 3269 | 4872 | 4872 | 0/7 | ok |
| Q030 | unicode-off | 8421.764 | 8663.759 | 9919.526 | 9919.526 | 1264 | 2251 | 0 | 0/7 | ok |
| Q034 | unicode-off | 3.795 | 4.229 | 28.069 | 28.069 | 1 | 1 | 0 | 0/7 | ok |
| Q035 | unicode-off | 21.446 | 22.942 | 43.856 | 43.856 | 124 | 124 | 0 | 0/7 | ok |
| Q054 | unicode-off | 1517.565 | 1527.217 | 1697.377 | 1697.377 | 66 | 66 | 0 | 0/7 | ok |
| Q081 | unicode-off | 3251.518 | 3278.982 | 3539.412 | 3539.412 | 888 | 888 | 888 | 0/7 | ok |
| Q116 | unicode-off | 186.800 | 190.264 | 243.091 | 243.091 | 70428 | 70428 | 0 | 0/7 | ok |
| Q136 | unicode-off | 2951.570 | 3132.918 | 5135.963 | 5135.963 | 133948 | 133948 | 0 | 0/7 | ok |
| Q144 | unicode-off | 64304.965 | 65815.064 | 67370.870 | 67370.870 | 3939 | 8570 | 8570 | 0/7 | ok |

## Index construction

- "unicode": build 0.674 s; parses 148486; resident 585 MiB; peak resident 626 MiB; estimated index 102 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
