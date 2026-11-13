# Musenalm query checkpoint

Code revision: `7a9dee532e75bf167b6e4eabfd52277c79bb4df1`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q214 | ascii-off | 2570.951 | 3066.457 | 6366.005 | 133948 | 133948 | 0 | 14/15 | 0/15 | ok |

## Index construction

- "ascii": build 1.039 s; snapshot write 0.251 s; parses 148486; resident 480 MiB; peak resident 503 MiB; estimated index 66 MiB; snapshot 124 MiB; fresh-process restore min/median/max 0.608/0.638/0.680 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
