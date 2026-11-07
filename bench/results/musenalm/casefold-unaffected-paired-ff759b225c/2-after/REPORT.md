# Musenalm query checkpoint

Code revision: `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. Repetitions: 31. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 817.554 | 934.149 | 1915.668 | 11672 | 11672 | 0 | 30/31 | 0/31 | ok |
| Q085 | ascii-off | 4020.803 | 5146.205 | 15804.345 | 4925 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q099 | ascii-off | 1835.398 | 2464.027 | 10205.461 | 59586 | 59586 | 0 | 30/31 | 0/31 | ok |
| Q107 | ascii-off | 10613.936 | 14300.817 | 22087.007 | 2684 | 4010 | 4010 | 30/31 | 0/31 | ok |
| Q112 | ascii-off | 3134.141 | 4454.759 | 11279.203 | 25 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q056 | unicode-off | 845.385 | 974.005 | 2755.865 | 11672 | 11672 | 0 | 30/31 | 0/31 | ok |
| Q085 | unicode-off | 4041.040 | 5468.680 | 11900.264 | 4925 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q099 | unicode-off | 1882.486 | 2984.779 | 18851.712 | 59586 | 59586 | 0 | 30/31 | 0/31 | ok |
| Q107 | unicode-off | 10822.135 | 12924.674 | 27649.376 | 2684 | 4010 | 4010 | 30/31 | 0/31 | ok |
| Q112 | unicode-off | 3261.929 | 4317.674 | 11701.702 | 25 | 4928 | 4928 | 30/31 | 0/31 | ok |

## Index construction

- "ascii": build 11.877 s; snapshot write 0.269 s; parses 148486; resident 251 MiB; peak resident 341 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.484/0.487/0.533 s over 5 runs.
- "unicode": build 11.746 s; snapshot write 0.283 s; parses 148486; resident 319 MiB; peak resident 385 MiB; estimated index 68 MiB; snapshot 108 MiB; fresh-process restore min/median/max 0.482/0.501/0.525 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
