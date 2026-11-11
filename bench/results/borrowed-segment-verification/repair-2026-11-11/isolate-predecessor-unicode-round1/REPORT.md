# Musenalm query checkpoint

Code revision: `5166876c7228f4fe2f8f064861cdc8a88275ad18`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | unicode-off | 6943.162 | 7426.100 | 10922.352 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q186 | unicode-off | 5003.105 | 5451.808 | 8676.901 | 627 | 1627 | 1627 | 20/21 | 0/21 | ok |

## Index construction

- "unicode": build 2.740 s; snapshot write 0.463 s; parses 148486; resident 463 MiB; peak resident 555 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.368/1.379/1.456 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
