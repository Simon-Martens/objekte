# Musenalm query checkpoint

Code revision: `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 10951.359 | 11616.498 | 15259.908 | 3269 | 4872 | 4872 | 10/11 | 0/11 | ok |
| Q014 | ascii-off | 17541.603 | 21299.143 | 31350.026 | 1742 | 2896 | 2896 | 10/11 | 0/11 | ok |
| Q026 | ascii-off | 105436.203 | 123670.214 | 133845.291 | 18825 | 80994 | 80994 | 10/11 | 0/11 | ok |
| Q078 | ascii-off | 51728.067 | 75554.105 | 84939.408 | 7632 | 80994 | 80994 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 2895.782 | 9353.063 | 24158.027 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | ascii-off | 2868.204 | 3352.157 | 8381.883 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 4096.853 | 4943.070 | 8135.111 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q001 | unicode-off | 10946.053 | 11416.090 | 13225.924 | 3269 | 4872 | 4872 | 10/11 | 0/11 | ok |
| Q014 | unicode-off | 17022.079 | 20953.306 | 28268.729 | 1742 | 2896 | 2896 | 10/11 | 0/11 | ok |
| Q026 | unicode-off | 113594.130 | 131378.935 | 139674.285 | 18825 | 81159 | 81159 | 10/11 | 0/11 | ok |
| Q078 | unicode-off | 53314.947 | 68918.673 | 88956.212 | 7632 | 81159 | 81159 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 2007.220 | 2952.640 | 5908.421 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | unicode-off | 3230.631 | 4373.948 | 6209.553 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 4403.649 | 7146.392 | 13093.602 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.887 s; snapshot write 0.268 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.484/0.486/0.525 s over 5 runs.
- "unicode": build 12.033 s; snapshot write 0.293 s; parses 148486; resident 323 MiB; peak resident 390 MiB; estimated index 68 MiB; snapshot 108 MiB; fresh-process restore min/median/max 0.490/0.501/0.564 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
