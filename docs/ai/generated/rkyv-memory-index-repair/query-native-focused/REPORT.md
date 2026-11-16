# Musenalm query checkpoint

Code revision: `45a81a85be5f8a122a3272ce3369ef2ed81632d8`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12352.702 | 13106.449 | 17599.590 | 17599.590 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q027 | ascii-off | 1325.172 | 1356.569 | 1641.642 | 1641.642 | 215 | 578 | 578 | 0/3 | ok |
| Q043 | ascii-off | 5748.485 | 5890.040 | 6404.410 | 6404.410 | 3022 | 3022 | 0 | 0/3 | ok |
| Q119 | ascii-off | 4290.186 | 4540.429 | 4960.201 | 4960.201 | 133948 | 133948 | 0 | 0/3 | ok |
| Q001 | unicode-off | 12453.360 | 13511.249 | 18177.384 | 18177.384 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q027 | unicode-off | 1302.209 | 1324.912 | 1615.094 | 1615.094 | 215 | 578 | 578 | 0/3 | ok |
| Q043 | unicode-off | 5751.612 | 5776.644 | 6438.111 | 6438.111 | 3022 | 3022 | 0 | 0/3 | ok |
| Q119 | unicode-off | 4611.134 | 4713.380 | 4995.110 | 4995.110 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.653 s; parses 148486; resident 588 MiB; peak resident 628 MiB; estimated index 0 MiB.
- "unicode": build 0.685 s; parses 148486; resident 706 MiB; peak resident 724 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
