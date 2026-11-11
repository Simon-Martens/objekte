# Musenalm query checkpoint

Code revision: `47931f0cdfefb860b0a0cac17ae06e068daf8a1e`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 592663.457 | 619245.516 | 632749.231 | 65548 | 148486 | 148486 | 20/21 | 0/21 | ok |
| Q145 | ascii-off | 212045.717 | 218465.314 | 245851.266 | 40947 | 44373 | 44373 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.566 s; snapshot write 0.445 s; parses 148486; resident 518 MiB; peak resident 544 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.365/1.371/1.411 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
