# Musenalm query checkpoint

Code revision: `697e0f5274ce8697da6343c0622e460f69eb70cc`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12258.726 | 15103.232 | 18377.597 | 18377.597 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | ascii-off | 4585.514 | 4598.071 | 4988.562 | 4988.562 | 133948 | 133948 | 0 | 0/3 | ok |
| Q001 | unicode-off | 11946.945 | 12811.604 | 18639.166 | 18639.166 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | unicode-off | 4559.245 | 4560.759 | 5053.864 | 5053.864 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": open/build 0.640 s; parses 148486; resident 585 MiB; peak resident 626 MiB; estimated index 105 MiB.
- "unicode": open/build 0.697 s; parses 148486; resident 882 MiB; peak resident 900 MiB; estimated index 109 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
