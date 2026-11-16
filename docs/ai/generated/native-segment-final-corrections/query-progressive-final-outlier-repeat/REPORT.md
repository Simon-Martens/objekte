# Musenalm query checkpoint

Code revision: `170eaeca03efa4cbc591a43fc3e611dd333b6ad2`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q119 | ascii-off | 4459.757 | 4488.712 | 5017.773 | 5017.773 | 133948 | 133948 | 0 | 0/3 | ok |
| Q119 | unicode-off | 4688.568 | 4704.291 | 4980.238 | 4980.238 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": open/build 0.649 s; parses 148486; resident 610 MiB; peak resident 651 MiB; estimated index 105 MiB.
- "unicode": open/build 0.724 s; parses 148486; resident 895 MiB; peak resident 914 MiB; estimated index 109 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
