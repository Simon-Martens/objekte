# Musenalm query checkpoint

Code revision: `093726e6b4b8168499133bd35086188e50e93b64`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12950.690 | 13333.743 | 17790.377 | 17790.377 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q042 | ascii-off | 99.928 | 102.898 | 165.708 | 165.708 | 1520 | 1520 | 0 | 0/3 | ok |
| Q083 | ascii-off | 6520.054 | 6734.481 | 7446.326 | 7446.326 | 4040 | 4928 | 4928 | 0/3 | ok |
| Q136 | ascii-off | 2716.948 | 3828.845 | 4573.210 | 4573.210 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.736 s; parses 148486; resident 592 MiB; peak resident 631 MiB; estimated index 105 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
