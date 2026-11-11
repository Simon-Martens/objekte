# Musenalm query checkpoint

Code revision: `47931f0cdfefb860b0a0cac17ae06e068daf8a1e`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 610047.611 | 648152.799 | 665113.230 | 65548 | 148486 | 148486 | 20/21 | 0/21 | ok |
| Q145 | ascii-off | 228751.741 | 235941.216 | 241820.057 | 40947 | 44373 | 44373 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.628 s; snapshot write 0.434 s; parses 148486; resident 456 MiB; peak resident 536 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.352/1.372/1.405 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
