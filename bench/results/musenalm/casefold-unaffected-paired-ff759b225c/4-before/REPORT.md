# Musenalm query checkpoint

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Repetitions: 31. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 762.686 | 881.413 | 2720.542 | 11672 | 11672 | 0 | 30/31 | 0/31 | ok |
| Q085 | ascii-off | 4011.525 | 5402.015 | 13642.795 | 4925 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q099 | ascii-off | 1821.533 | 3809.061 | 19683.335 | 59586 | 59586 | 0 | 30/31 | 0/31 | ok |
| Q107 | ascii-off | 10566.865 | 12749.457 | 24885.074 | 2684 | 4010 | 4010 | 30/31 | 0/31 | ok |
| Q112 | ascii-off | 3102.499 | 4220.822 | 15904.908 | 25 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q056 | unicode-off | 831.452 | 896.032 | 2520.596 | 11672 | 11672 | 0 | 30/31 | 0/31 | ok |
| Q085 | unicode-off | 4118.515 | 5084.190 | 14324.096 | 4925 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q099 | unicode-off | 1870.175 | 3073.502 | 16502.548 | 59586 | 59586 | 0 | 30/31 | 0/31 | ok |
| Q107 | unicode-off | 10768.569 | 12504.244 | 24536.122 | 2684 | 4010 | 4010 | 30/31 | 0/31 | ok |
| Q112 | unicode-off | 2767.995 | 4006.014 | 11640.030 | 25 | 4928 | 4928 | 30/31 | 0/31 | ok |

## Index construction

- "ascii": build 11.134 s; snapshot write 0.278 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.488/0.493/0.536 s over 5 runs.
- "unicode": build 12.131 s; snapshot write 0.316 s; parses 148486; resident 332 MiB; peak resident 403 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.509/0.521/0.556 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
