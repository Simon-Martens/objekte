# Musenalm query checkpoint

Code revision: `00f501902c8aee2a66da5cb829b56ab89dbf0ac8`. Repetitions: 21. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q190 | ascii-off | 19918.170 | 23243.267 | 62411.712 | 3821 | 3821 | 3821 | 0/21 | 0/21 | ok |
| Q190 | unicode-off | 19596.786 | 23270.959 | 60819.371 | 3821 | 3821 | 3821 | 0/21 | 0/21 | ok |

## Index construction

- "ascii": build 2.582 s; snapshot write 0.545 s; parses 148486; resident 535 MiB; peak resident 560 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.332/1.345/1.439 s over 5 runs.
- "unicode": build 2.580 s; snapshot write 0.546 s; parses 148486; resident 724 MiB; peak resident 724 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.344/1.360/1.437 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
