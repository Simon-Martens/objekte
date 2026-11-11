# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 1361.307 | 1381.966 | 3945.122 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 16376.153 | 18953.727 | 21915.530 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | ascii-off | 352.563 | 721.816 | 1906.672 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | ascii-off | 250016.017 | 255577.595 | 263043.781 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q056 | unicode-off | 1353.303 | 1476.864 | 3924.012 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 15991.381 | 17502.570 | 22940.125 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | unicode-off | 329.229 | 348.544 | 1933.322 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | unicode-off | 248089.058 | 253399.402 | 261955.747 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.485 s; snapshot write 0.431 s; parses 148486; resident 456 MiB; peak resident 534 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.337/1.341/1.391 s over 5 runs.
- "unicode": build 2.583 s; snapshot write 0.451 s; parses 148486; resident 732 MiB; peak resident 732 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.326/1.330/1.416 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
