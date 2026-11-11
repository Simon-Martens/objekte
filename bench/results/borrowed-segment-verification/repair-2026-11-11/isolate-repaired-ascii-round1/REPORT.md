# Musenalm query checkpoint

Code revision: `49ac3f503ac63f8303f85122a1ec4f0043949cda`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 606801.084 | 639160.844 | 658381.535 | 65548 | 148486 | 148486 | 20/21 | 0/21 | ok |
| Q145 | ascii-off | 222345.128 | 227380.764 | 235391.973 | 40947 | 44373 | 44373 | 20/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.602 s; snapshot write 0.435 s; parses 148486; resident 506 MiB; peak resident 543 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.372/1.374/1.420 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
