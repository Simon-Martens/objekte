# Musenalm query checkpoint

Code revision: `23d38d5e3fe5559879fc26a0e8bf0a803f2e51bb`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q141 | ascii-off | 4953.380 | 5111.222 | 7746.928 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 4747.646 | 5480.187 | 9901.243 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | ascii-off | 1862.509 | 2161.450 | 3427.857 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | ascii-off | 3320.782 | 3509.287 | 5185.190 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 4635.613 | 6205.822 | 8283.601 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 4679.955 | 4893.757 | 8011.400 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q209 | unicode-off | 1789.442 | 1999.913 | 2802.694 | 63520 | 63520 | 0 | 6/7 | 0/7 | ok |
| Q214 | unicode-off | 2910.361 | 3095.380 | 4093.784 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 1.035 s; snapshot write 0.297 s; parses 148486; resident 485 MiB; peak resident 508 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.617/0.631/0.711 s over 5 runs.
- "unicode": build 0.993 s; snapshot write 0.297 s; parses 148486; resident 624 MiB; peak resident 624 MiB; estimated index 69 MiB; snapshot 126 MiB; fresh-process restore min/median/max 0.622/0.642/0.705 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
