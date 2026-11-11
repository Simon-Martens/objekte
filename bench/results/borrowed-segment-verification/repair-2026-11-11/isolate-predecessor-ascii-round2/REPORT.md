# Musenalm query checkpoint

Code revision: `5166876c7228f4fe2f8f064861cdc8a88275ad18`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 870024.446 | 903744.382 | 922551.486 | 65548 | 148486 | 148486 | 20/21 | 0/21 | ok |
| Q145 | ascii-off | 289846.919 | 299862.686 | 307719.126 | 40947 | 44373 | 44373 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.560 s; snapshot write 0.444 s; parses 148486; resident 460 MiB; peak resident 543 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.368/1.382/1.458 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
