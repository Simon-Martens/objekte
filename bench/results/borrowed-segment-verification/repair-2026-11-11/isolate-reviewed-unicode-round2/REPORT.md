# Musenalm query checkpoint

Code revision: `47931f0cdfefb860b0a0cac17ae06e068daf8a1e`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q072 | unicode-off | 6694.194 | 7200.787 | 11037.017 | 1287 | 2251 | 2251 | 20/21 | 0/21 | ok |
| Q186 | unicode-off | 4814.430 | 5326.593 | 8033.882 | 627 | 1627 | 1627 | 20/21 | 0/21 | ok |

## Index construction

- "unicode": build 2.710 s; snapshot write 0.447 s; parses 148486; resident 546 MiB; peak resident 572 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.357/1.384/1.450 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
