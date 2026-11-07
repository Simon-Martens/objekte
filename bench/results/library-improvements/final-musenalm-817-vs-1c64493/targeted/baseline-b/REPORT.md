# Musenalm query checkpoint

Code revision: `81788174898c27b2c2c367319bad20ecee450562`. Repetitions: 10. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q074 | ascii-off | 4344.968 | 6030.904 | 10277.206 | 4927 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q116 | ascii-off | 140233.464 | 159477.427 | 182555.792 | 70428 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q117 | ascii-off | 133179.015 | 162118.012 | 178248.687 | 63520 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q158 | ascii-off | 5267.123 | 9215.227 | 14923.630 | 3106 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q187 | ascii-off | 256782.644 | 286747.335 | 312410.437 | 133948 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q074 | unicode-off | 4576.392 | 5982.795 | 10469.853 | 4927 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q116 | unicode-off | 124615.099 | 173382.958 | 180859.469 | 70428 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q117 | unicode-off | 137392.549 | 148406.529 | 156468.841 | 63520 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q158 | unicode-off | 6214.202 | 8688.964 | 18452.089 | 3106 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q187 | unicode-off | 271715.297 | 300045.509 | 324976.312 | 133948 | 133948 | 133948 | 9/10 | 0/10 | ok |

## Index construction

- "ascii": build 1.228 s; snapshot write 0.208 s; parses 148486; resident 505 MiB; peak resident 596 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.539/0.543/0.557 s over 5 runs.
- "unicode": build 1.233 s; snapshot write 0.207 s; parses 148486; resident 693 MiB; peak resident 763 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.541/0.553/0.560 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
