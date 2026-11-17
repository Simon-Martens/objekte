# Musenalm query checkpoint

Code revision: `697e0f5274ce8697da6343c0622e460f69eb70cc`. Repetitions: 3. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 12088.489 | 12455.369 | 13851.450 | 13851.450 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | ascii-off | 4591.284 | 4704.087 | 5119.918 | 5119.918 | 133948 | 133948 | 0 | 0/3 | ok |
| Q001 | unicode-off | 12652.695 | 12785.685 | 17282.867 | 17282.867 | 3269 | 4872 | 4872 | 0/3 | ok |
| Q119 | unicode-off | 4594.113 | 4607.051 | 9537.908 | 9537.908 | 133948 | 133948 | 0 | 0/3 | ok |

## Index construction

- "ascii": build 0.652 s; parses 148486; resident 589 MiB; peak resident 630 MiB; estimated index 0 MiB.
- "unicode": build 0.669 s; parses 148486; resident 699 MiB; peak resident 716 MiB; estimated index 0 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
