# Musenalm query checkpoint

Code revision: `d78e6ef0f00bd447742a229b1f2afafae7f3d396`. Repetitions: 6. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q035 | ascii-off | 30.280 | 32.887 | 118.341 | 124 | 124 | 0 | 5/6 | 0/6 | ok |
| Q045 | ascii-off | 140.749 | 191.030 | 321.111 | 1107 | 1107 | 0 | 5/6 | 0/6 | ok |
| Q084 | ascii-off | 159.841 | 200.642 | 234.577 | 4922 | 4922 | 0 | 5/6 | 0/6 | ok |
| Q127 | ascii-off | 52.004 | 59.447 | 80.266 | 203 | 203 | 0 | 5/6 | 0/6 | ok |
| Q189 | ascii-off | 5122.137 | 5384.503 | 15814.126 | 7 | 203 | 203 | 5/6 | 0/6 | ok |
| Q190 | ascii-off | 73261.704 | 77606.524 | 86426.573 | 3821 | 3821 | 3821 | 5/6 | 0/6 | ok |
| Q194 | ascii-off | 280.504 | 372.916 | 855.006 | 7 | 203 | 203 | 6/6 | 0/6 | ok |

## Index construction

- "ascii": build 1.160 s; snapshot write 0.235 s; parses 148486; resident 513 MiB; peak resident 604 MiB; estimated index 65 MiB; snapshot 111 MiB; fresh-process restore min/median/max 0.547/0.554/0.563 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
