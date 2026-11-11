# Musenalm query checkpoint

Code revision: `5166876c7228f4fe2f8f064861cdc8a88275ad18`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | unicode-off | 6757.833 | 7243.075 | 9735.180 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q186 | unicode-off | 4858.063 | 5190.648 | 7838.144 | 627 | 1627 | 1627 | 20/21 | 0/21 | ok |

## Index construction

- "unicode": build 2.727 s; snapshot write 0.447 s; parses 148486; resident 554 MiB; peak resident 580 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.356/1.363/1.411 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
