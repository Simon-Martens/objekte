# Musenalm query checkpoint

Code revision: `d8e3b22f850811eb58f26716c2db819016612e96`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q056 | ascii-off | 1371.676 | 1384.672 | 4013.069 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | ascii-off | 14312.167 | 14867.079 | 21187.663 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | ascii-off | 333.637 | 392.618 | 1603.482 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | ascii-off | 250444.482 | 258858.889 | 287320.347 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q056 | unicode-off | 1380.593 | 1397.465 | 3908.412 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q069 | unicode-off | 14135.626 | 14425.690 | 19405.997 | 4010 | 4010 | 4010 | 6/7 | 0/7 | ok |
| Q086 | unicode-off | 333.907 | 388.460 | 2000.668 | 4903 | 4903 | 0 | 6/7 | 0/7 | ok |
| Q160 | unicode-off | 250617.739 | 256718.087 | 282119.045 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.626 s; snapshot write 0.426 s; parses 148486; resident 534 MiB; peak resident 560 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.323/1.331/1.428 s over 5 runs.
- "unicode": build 2.533 s; snapshot write 0.443 s; parses 148486; resident 715 MiB; peak resident 745 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.327/1.334/1.384 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
