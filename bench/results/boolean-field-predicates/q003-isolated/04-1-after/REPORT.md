# Musenalm query checkpoint

Code revision: `d666524f7f667305d3d1c19593835be9d478f2e5`. Repetitions: 15. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 3582.474 | 3830.038 | 8008.623 | 2866 | 2948 | 2948 | 0/15 | 0/15 | ok |

## Index construction

- "ascii": build 0.959 s; snapshot write 0.475 s; parses 148486; resident 583 MiB; peak resident 747 MiB; estimated index 66 MiB; snapshot 170 MiB; fresh-process restore min/median/max 0.000/0.000/0.000 s over 0 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
