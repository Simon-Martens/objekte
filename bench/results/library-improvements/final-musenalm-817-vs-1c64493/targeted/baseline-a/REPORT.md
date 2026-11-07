# Musenalm query checkpoint

Code revision: `81788174898c27b2c2c367319bad20ecee450562`. Repetitions: 10. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q074 | ascii-off | 4607.147 | 4876.972 | 5652.343 | 4927 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q116 | ascii-off | 129623.306 | 162134.567 | 173251.798 | 70428 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q117 | ascii-off | 135654.220 | 151286.350 | 167272.316 | 63520 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q158 | ascii-off | 6706.830 | 9608.771 | 11523.545 | 3106 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q187 | ascii-off | 274716.852 | 284484.548 | 308755.605 | 133948 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q074 | unicode-off | 5241.800 | 6491.598 | 9357.664 | 4927 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q116 | unicode-off | 126714.550 | 157450.491 | 189868.480 | 70428 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q117 | unicode-off | 147584.319 | 152722.644 | 179740.461 | 63520 | 133948 | 133948 | 9/10 | 0/10 | ok |
| Q158 | unicode-off | 7139.078 | 9663.487 | 18268.466 | 3106 | 4928 | 4928 | 9/10 | 0/10 | ok |
| Q187 | unicode-off | 262788.512 | 286761.473 | 317369.737 | 133948 | 133948 | 133948 | 9/10 | 0/10 | ok |

## Index construction

- "ascii": build 1.218 s; snapshot write 0.216 s; parses 148486; resident 493 MiB; peak resident 585 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.540/0.542/0.560 s over 5 runs.
- "unicode": build 1.254 s; snapshot write 0.205 s; parses 148486; resident 732 MiB; peak resident 802 MiB; estimated index 68 MiB; snapshot 114 MiB; fresh-process restore min/median/max 0.546/0.555/0.572 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
