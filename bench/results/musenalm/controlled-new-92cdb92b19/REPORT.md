# Musenalm query checkpoint

Code revision: `92cdb92b19200cb0005af23b66aeb3cd1b514c4b`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 209024.730 | 244479.780 | 252784.464 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q007 | ascii-off | 245594.178 | 280346.229 | 289911.241 | 147798 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q014 | ascii-off | 16262.400 | 17346.946 | 30770.591 | 1742 | 2896 | 2896 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 1915.315 | 2572.856 | 6751.194 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | ascii-off | 14889.001 | 16283.804 | 34717.169 | 0 | 22185 | 22185 | 10/11 | 0/11 | ok |
| Q148 | ascii-off | 6312.098 | 8064.692 | 10900.252 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 194467.212 | 242509.011 | 246412.247 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q007 | unicode-off | 246484.183 | 290803.315 | 302795.476 | 147798 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q014 | unicode-off | 10036.134 | 10393.125 | 11309.457 | 1742 | 1744 | 1744 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 2648.160 | 10066.474 | 14392.447 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q100 | unicode-off | 12509.505 | 14850.036 | 27169.778 | 0 | 18227 | 18227 | 10/11 | 0/11 | ok |
| Q148 | unicode-off | 4349.295 | 5825.578 | 8624.597 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.075 s; snapshot write 0.660 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.601/0.607/0.751 s over 5 runs.
- "unicode": build 12.518 s; snapshot write 0.668 s; parses 148486; resident 340 MiB; peak resident 411 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.674/0.677/0.702 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
