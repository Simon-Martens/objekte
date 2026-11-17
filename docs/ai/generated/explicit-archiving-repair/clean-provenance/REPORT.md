# Musenalm query checkpoint

Code revision: `7cb23fb7a24ce413935a42c9d391dd15fa82d151`. Repetitions: 1. Failures: 0.

| Case | Profile | Min µs | Median µs | p95 µs | Max µs | Matches | Candidates | Parses | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q001 | ascii-off | 9055.922 | 9055.922 | 9055.922 | 9055.922 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | ascii-off | 11938.767 | 11938.767 | 11938.767 | 11938.767 | 133948 | 133948 | 0 | 0/1 | ok |
| Q001 | unicode-off | 34254.621 | 34254.621 | 34254.621 | 34254.621 | 3269 | 4872 | 4872 | 0/1 | ok |
| Q119 | unicode-off | 11492.491 | 11492.491 | 11492.491 | 11492.491 | 133948 | 133948 | 0 | 0/1 | ok |

## Index construction

- "ascii": native build 764.182 ms; combined 873.215 ms; explicit archive 108.978 ms; parses 148486; native-ready RSS 665 MiB; archived RSS 885 MiB; peak resident 885 MiB; estimated index 99 MiB; rkyv archive 142 MiB.
- "unicode": native build 818.562 ms; combined 916.698 ms; explicit archive 98.069 ms; parses 148486; native-ready RSS 1545 MiB; archived RSS 1606 MiB; peak resident 1606 MiB; estimated index 103 MiB; rkyv archive 142 MiB.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
