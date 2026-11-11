# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 1375.945 | 1406.912 | 4028.868 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 14381.655 | 14871.465 | 18906.275 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | ascii-off | 333.266 | 348.254 | 1483.357 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | ascii-off | 265296.649 | 273981.214 | 299282.571 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q056 | unicode-off | 1429.095 | 1512.712 | 3948.296 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 14186.228 | 14699.062 | 18107.695 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | unicode-off | 332.895 | 355.177 | 1523.562 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | unicode-off | 251740.606 | 254001.903 | 276042.055 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.501 s; snapshot write 0.446 s; parses 148486; resident 532 MiB; peak resident 558 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.336/1.337/1.431 s over 5 runs.
- "unicode": build 2.540 s; snapshot write 0.449 s; parses 148486; resident 685 MiB; peak resident 685 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.355/1.362/1.438 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
