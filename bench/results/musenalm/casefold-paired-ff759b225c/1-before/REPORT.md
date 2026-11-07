# Musenalm query checkpoint

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 10688.160 | 11801.849 | 15458.997 | 3269 | 4872 | 4872 | 10/11 | 0/11 | ok |
| Q014 | ascii-off | 17016.633 | 20260.723 | 27892.467 | 1742 | 2896 | 2896 | 10/11 | 0/11 | ok |
| Q026 | ascii-off | 105521.574 | 122210.205 | 130467.383 | 18825 | 80994 | 80994 | 10/11 | 0/11 | ok |
| Q078 | ascii-off | 53368.037 | 62694.708 | 85697.668 | 7632 | 80994 | 80994 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 4131.555 | 8750.405 | 17037.084 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | ascii-off | 2901.794 | 3468.141 | 6154.945 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 2999.984 | 3367.010 | 6086.294 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q001 | unicode-off | 6325.408 | 6674.755 | 8096.550 | 3269 | 3424 | 3424 | 10/11 | 0/11 | ok |
| Q014 | unicode-off | 10537.108 | 12961.002 | 20036.254 | 1742 | 1744 | 1744 | 10/11 | 0/11 | ok |
| Q026 | unicode-off | 30707.335 | 33460.056 | 56851.993 | 18825 | 19419 | 19419 | 10/11 | 0/11 | ok |
| Q078 | unicode-off | 17873.248 | 23140.437 | 42251.038 | 7632 | 19419 | 19419 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 2296.096 | 10239.388 | 16134.730 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | unicode-off | 2677.362 | 3216.044 | 6474.320 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 2808.899 | 3220.623 | 5355.029 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.011 s; snapshot write 0.432 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.486/0.495/0.600 s over 5 runs.
- "unicode": build 12.154 s; snapshot write 0.311 s; parses 148486; resident 336 MiB; peak resident 408 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.504/0.516/0.574 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
