# Musenalm query checkpoint

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 188803.182 | 227905.103 | 233373.627 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q022 | ascii-off | 3073.104 | 3244.522 | 4943.409 | 1493 | 1504 | 1504 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 98171.785 | 123466.326 | 134193.462 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q052 | ascii-off | 4202.347 | 7605.237 | 23453.232 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q068 | ascii-off | 3080.927 | 3545.676 | 6421.590 | 1 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q069 | ascii-off | 4693.464 | 8212.889 | 16602.439 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q096 | ascii-off | 2178.845 | 3397.653 | 12005.713 | 70390 | 70390 | 0 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 1997.725 | 2885.438 | 7404.934 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | ascii-off | 14122.480 | 21189.177 | 35014.185 | 0 | 22185 | 22185 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3290.932 | 3988.941 | 6898.718 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 195168.222 | 240764.973 | 249407.825 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q022 | unicode-off | 2803.542 | 3067.030 | 4243.355 | 1493 | 1503 | 1503 | 10/11 | 0/11 | ok |
| Q025 | unicode-off | 97768.357 | 124878.066 | 131286.027 | 60319 | 61266 | 61266 | 10/11 | 0/11 | ok |
| Q052 | unicode-off | 3694.025 | 6021.926 | 20827.060 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q068 | unicode-off | 3052.070 | 3633.666 | 5196.100 | 1 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q069 | unicode-off | 6610.924 | 8992.306 | 22253.299 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q096 | unicode-off | 2314.957 | 3192.450 | 8060.682 | 70390 | 70390 | 0 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 1937.797 | 2332.299 | 6896.018 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | unicode-off | 12138.537 | 16142.135 | 29148.745 | 0 | 18227 | 18227 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 3250.339 | 4338.006 | 12404.157 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.001 s; snapshot write 0.280 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.488/0.492/0.533 s over 5 runs.
- "unicode": build 12.122 s; snapshot write 0.399 s; parses 148486; resident 336 MiB; peak resident 407 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.503/0.519/0.563 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
