# Musenalm query checkpoint

Code revision: `5166876c7228f4fe2f8f064861cdc8a88275ad18`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 903680.917 | 938410.181 | 963594.858 | 65548 | 148486 | 148486 | 20/21 | 0/21 | ok |
| Q145 | ascii-off | 307134.227 | 317188.521 | 322064.384 | 40947 | 44373 | 44373 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.543 s; snapshot write 0.437 s; parses 148486; resident 527 MiB; peak resident 553 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.321/1.331/1.417 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
