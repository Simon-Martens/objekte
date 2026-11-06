# Musenalm query checkpoint

Code revision: `ff759b225c6bc4bfdd614c542fe38fe017a900b6`. Repetitions: 31. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 811.323 | 875.752 | 2660.762 | 11672 | 11672 | 0 | 30/31 | 0/31 | ok |
| Q085 | ascii-off | 3899.342 | 5202.120 | 15226.165 | 4925 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q099 | ascii-off | 1907.668 | 2946.213 | 17920.904 | 59586 | 59586 | 0 | 30/31 | 0/31 | ok |
| Q107 | ascii-off | 10985.351 | 13092.516 | 26581.350 | 2684 | 4010 | 4010 | 30/31 | 0/31 | ok |
| Q112 | ascii-off | 3060.378 | 4499.276 | 11360.833 | 25 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q056 | unicode-off | 837.278 | 904.686 | 1938.684 | 11672 | 11672 | 0 | 30/31 | 0/31 | ok |
| Q085 | unicode-off | 3966.278 | 5735.420 | 13706.297 | 4925 | 4928 | 4928 | 30/31 | 0/31 | ok |
| Q099 | unicode-off | 1915.614 | 2731.312 | 15038.376 | 59586 | 59586 | 0 | 30/31 | 0/31 | ok |
| Q107 | unicode-off | 10458.169 | 12743.025 | 21364.130 | 2684 | 4010 | 4010 | 30/31 | 0/31 | ok |
| Q112 | unicode-off | 3023.198 | 4604.725 | 13229.947 | 25 | 4928 | 4928 | 30/31 | 0/31 | ok |

## Index construction

- "ascii": build 11.858 s; snapshot write 0.270 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.484/0.490/0.521 s over 5 runs.
- "unicode": build 12.028 s; snapshot write 0.277 s; parses 148486; resident 320 MiB; peak resident 386 MiB; estimated index 68 MiB; snapshot 108 MiB; fresh-process restore min/median/max 0.485/0.508/0.532 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
