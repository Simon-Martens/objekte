# Musenalm query checkpoint

Code revision: `20261604bebf949131441c8262f1f442bce4207c`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 10390.506 | 11289.423 | 15207.531 | 3269 | 4872 | 4872 | 10/11 | 0/11 | ok |
| Q014 | ascii-off | 17074.772 | 19140.281 | 28232.968 | 1742 | 2896 | 2896 | 10/11 | 0/11 | ok |
| Q026 | ascii-off | 96670.818 | 123204.194 | 134054.090 | 18825 | 80994 | 80994 | 10/11 | 0/11 | ok |
| Q078 | ascii-off | 49129.369 | 68897.372 | 89310.526 | 7632 | 80994 | 80994 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 1888.039 | 2120.960 | 4490.655 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | ascii-off | 3026.910 | 3969.586 | 8762.511 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 4305.007 | 5271.569 | 12257.077 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q001 | unicode-off | 6291.591 | 6736.020 | 7483.840 | 3269 | 3424 | 3424 | 10/11 | 0/11 | ok |
| Q014 | unicode-off | 10370.827 | 13312.704 | 19825.283 | 1742 | 1744 | 1744 | 10/11 | 0/11 | ok |
| Q026 | unicode-off | 29276.488 | 35729.934 | 57379.118 | 18825 | 19419 | 19419 | 10/11 | 0/11 | ok |
| Q078 | unicode-off | 17277.292 | 21299.998 | 34737.849 | 7632 | 19419 | 19419 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 4955.542 | 6506.219 | 14362.648 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | unicode-off | 3107.966 | 3859.521 | 4875.135 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 3011.445 | 3305.284 | 6177.110 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 10.972 s; snapshot write 0.269 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.485/0.489/0.543 s over 5 runs.
- "unicode": build 12.017 s; snapshot write 0.309 s; parses 148486; resident 336 MiB; peak resident 408 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.496/0.502/0.567 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
