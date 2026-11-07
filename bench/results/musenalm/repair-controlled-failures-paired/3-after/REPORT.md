# Musenalm query checkpoint

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 193550.087 | 229720.924 | 234541.662 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q022 | ascii-off | 2974.346 | 3308.042 | 3732.281 | 1493 | 1504 | 1504 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 94906.784 | 120158.643 | 126567.532 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q052 | ascii-off | 3905.546 | 10481.297 | 22739.405 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q068 | ascii-off | 3225.337 | 3928.150 | 5719.131 | 1 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q069 | ascii-off | 6738.741 | 8788.563 | 11083.342 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q096 | ascii-off | 2313.157 | 2594.715 | 12999.225 | 70390 | 70390 | 0 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 1958.957 | 3226.578 | 12002.065 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | ascii-off | 15707.320 | 25854.502 | 32981.862 | 0 | 22185 | 22185 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3063.258 | 3533.184 | 4545.709 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 190697.793 | 241326.394 | 247639.150 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q022 | unicode-off | 2884.875 | 3083.707 | 4024.704 | 1493 | 1503 | 1503 | 10/11 | 0/11 | ok |
| Q025 | unicode-off | 94926.644 | 113852.422 | 133684.685 | 60319 | 61266 | 61266 | 10/11 | 0/11 | ok |
| Q052 | unicode-off | 7116.366 | 10013.406 | 19053.989 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q068 | unicode-off | 2828.899 | 3145.937 | 5320.193 | 1 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q069 | unicode-off | 4391.991 | 5539.162 | 10550.942 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q096 | unicode-off | 2363.358 | 4573.042 | 33372.549 | 70390 | 70390 | 0 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 1850.365 | 2204.172 | 3742.958 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | unicode-off | 12768.335 | 14778.006 | 30439.718 | 0 | 18227 | 18227 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 3029.505 | 4734.817 | 7986.331 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 10.976 s; snapshot write 0.269 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.488/0.498/0.545 s over 5 runs.
- "unicode": build 12.134 s; snapshot write 0.309 s; parses 148486; resident 336 MiB; peak resident 409 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.524/0.535/0.561 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
