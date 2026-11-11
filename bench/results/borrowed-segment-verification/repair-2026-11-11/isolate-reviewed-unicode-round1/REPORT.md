# Musenalm query checkpoint

Code revision: `47931f0cdfefb860b0a0cac17ae06e068daf8a1e`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | unicode-off | 6546.316 | 6957.498 | 10319.599 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q186 | unicode-off | 4588.145 | 5146.123 | 9815.792 | 627 | 1627 | 1627 | 20/21 | 0/21 | ok |

## Index construction

- "unicode": build 2.723 s; snapshot write 0.453 s; parses 148486; resident 544 MiB; peak resident 569 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.376/1.379/1.471 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
