# Musenalm query checkpoint

Code revision: `9b8aa7131705d755c8ccebeefd7b9edf596ce9b2`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 189656.195 | 229618.732 | 236455.689 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 83657.631 | 94758.912 | 128134.424 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q028 | ascii-off | 4289.833 | 6080.385 | 15764.560 | 4177 | 4177 | 4177 | 10/11 | 0/11 | ok |
| Q052 | ascii-off | 4293.681 | 6565.408 | 12589.886 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q055 | ascii-off | 5626.036 | 11431.462 | 15344.727 | 112778 | 112778 | 0 | 10/11 | 0/11 | ok |
| Q069 | ascii-off | 4310.649 | 5582.299 | 7038.595 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q071 | ascii-off | 1471.163 | 1611.485 | 1996.671 | 1003 | 1627 | 1627 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 3097.038 | 9546.325 | 19823.442 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | ascii-off | 2874.192 | 3238.575 | 5402.028 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3217.972 | 4264.260 | 7330.904 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q148 | ascii-off | 5694.634 | 6965.793 | 9658.957 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 188976.985 | 239806.131 | 245547.170 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | unicode-off | 84167.941 | 90022.175 | 127319.243 | 60319 | 61266 | 61266 | 10/11 | 0/11 | ok |
| Q028 | unicode-off | 3778.172 | 4413.399 | 9443.795 | 4177 | 4177 | 4177 | 10/11 | 0/11 | ok |
| Q052 | unicode-off | 3303.230 | 4154.421 | 6303.674 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q055 | unicode-off | 5124.030 | 9344.794 | 13443.642 | 112778 | 112778 | 0 | 10/11 | 0/11 | ok |
| Q069 | unicode-off | 4545.043 | 5168.100 | 7625.194 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q071 | unicode-off | 1362.945 | 1423.213 | 1759.710 | 1003 | 1364 | 1364 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 2430.524 | 9925.852 | 20303.404 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | unicode-off | 3056.842 | 3432.675 | 4657.714 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 3191.732 | 4996.245 | 7461.645 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q148 | unicode-off | 4800.515 | 7989.620 | 15588.410 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.034 s; snapshot write 0.278 s; parses 148486; resident 251 MiB; peak resident 340 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.489/0.507/0.534 s over 5 runs.
- "unicode": build 12.158 s; snapshot write 0.385 s; parses 148486; resident 336 MiB; peak resident 407 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.513/0.535/0.571 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
