# Musenalm query checkpoint

Code revision: `d35ddde56e17114da35d1a76e776615262897409`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 13813.974 | 18643.407 | 22126.925 | 22126.925 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q042 | ascii-off | 88.598 | 95.207 | 162.203 | 162.203 | 1520 | 1520 | 0 | 0/3 | ok |
| Q083 | ascii-off | 6626.744 | 6864.764 | 7158.037 | 7158.037 | 4040 | 4928 | 4928 | 0/3 | ok |
| Q136 | ascii-off | 2764.177 | 3981.804 | 4689.740 | 4689.740 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.794 s; parses 148486; resident 621 MiB; peak resident 661 MiB; estimated index 105 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
