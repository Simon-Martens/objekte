# Musenalm query checkpoint

Code revision: `49ac3f503ac63f8303f85122a1ec4f0043949cda`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 611770.545 | 642872.281 | 657654.649 | 65548 | 148486 | 148486 | 20/21 | 0/21 | ok |
| Q145 | ascii-off | 222827.735 | 230143.477 | 244819.746 | 40947 | 44373 | 44373 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.571 s; snapshot write 0.447 s; parses 148486; resident 470 MiB; peak resident 534 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.367/1.392/1.434 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
