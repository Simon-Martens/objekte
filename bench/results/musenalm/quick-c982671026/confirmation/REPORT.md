# Musenalm query checkpoint

Code revision: `c982671026836112b24424c2f203d7634bda6160`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q071 | ascii-off | 4030.863 | 4170.031 | 5107.379 | 1003 | 1627 | 1627 | 2/3 | 0/3 | ok |

## Index construction

- "ascii": build 14.122 s; snapshot write 0.294 s; parses 148486; resident 242 MiB; peak resident 324 MiB; estimated index 65 MiB; snapshot 101 MiB; fresh-process restore min/median/max 0.542/0.550/0.566 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
