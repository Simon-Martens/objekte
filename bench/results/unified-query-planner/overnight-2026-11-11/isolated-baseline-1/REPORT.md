# Musenalm query checkpoint

Code revision: `05c00a3cbfa2f218ed063681eccbd9769386aef5`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q116 | ascii-off | 9058.679 | 11418.572 | 34247.731 | 70428 | 70428 | 0 | 20/21 | 0/21 | ok |
| Q128 | ascii-off | 372.430 | 400.392 | 865.957 | 3257 | 3257 | 0 | 20/21 | 0/21 | ok |
| Q130 | ascii-off | 1414.337 | 4578.181 | 6440.409 | 3257 | 3257 | 0 | 21/21 | 0/21 | ok |
| Q116 | unicode-off | 9578.685 | 11301.913 | 42224.597 | 70428 | 70428 | 0 | 20/21 | 0/21 | ok |
| Q128 | unicode-off | 524.966 | 891.053 | 1373.099 | 3257 | 3257 | 0 | 20/21 | 0/21 | ok |
| Q130 | unicode-off | 3127.816 | 4686.595 | 9064.771 | 3257 | 3257 | 0 | 21/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.555 s; snapshot write 0.539 s; parses 148486; resident 461 MiB; peak resident 542 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.308/1.318/1.393 s over 5 runs.
- "unicode": build 2.575 s; snapshot write 0.568 s; parses 148486; resident 715 MiB; peak resident 715 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.313/1.322/1.521 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
