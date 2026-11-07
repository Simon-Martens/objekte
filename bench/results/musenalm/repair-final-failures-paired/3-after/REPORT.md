# Musenalm query checkpoint

Code revision: `9b8aa7131705d755c8ccebeefd7b9edf596ce9b2`. Repetitions: 11. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q006 | ascii-off | 191297.723 | 228074.068 | 234754.423 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | ascii-off | 86113.961 | 94608.730 | 129212.332 | 60319 | 67748 | 67748 | 10/11 | 0/11 | ok |
| Q028 | ascii-off | 4230.001 | 5444.372 | 7884.640 | 4177 | 4177 | 4177 | 10/11 | 0/11 | ok |
| Q052 | ascii-off | 4389.251 | 6080.815 | 21055.819 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q055 | ascii-off | 3739.207 | 4422.009 | 10937.052 | 112778 | 112778 | 0 | 10/11 | 0/11 | ok |
| Q069 | ascii-off | 4793.795 | 6918.816 | 16262.821 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q071 | ascii-off | 1477.682 | 1757.389 | 4538.745 | 1003 | 1627 | 1627 | 10/11 | 0/11 | ok |
| Q099 | ascii-off | 2018.282 | 2305.868 | 12271.993 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | ascii-off | 3650.655 | 5231.096 | 8806.362 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | ascii-off | 3923.623 | 4607.335 | 10348.574 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q148 | ascii-off | 4074.166 | 4717.645 | 5049.149 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |
| Q006 | unicode-off | 200631.899 | 246907.249 | 258131.317 | 65548 | 148486 | 148486 | 10/11 | 0/11 | ok |
| Q025 | unicode-off | 86788.677 | 93669.283 | 134958.375 | 60319 | 61266 | 61266 | 10/11 | 0/11 | ok |
| Q028 | unicode-off | 4007.937 | 4385.673 | 4939.023 | 4177 | 4177 | 4177 | 10/11 | 0/11 | ok |
| Q052 | unicode-off | 3351.626 | 4423.068 | 7478.514 | 86764 | 86764 | 0 | 10/11 | 0/11 | ok |
| Q055 | unicode-off | 5442.066 | 9046.388 | 30617.926 | 112778 | 112778 | 0 | 10/11 | 0/11 | ok |
| Q069 | unicode-off | 4394.697 | 6040.559 | 8647.408 | 4010 | 4010 | 4010 | 10/11 | 0/11 | ok |
| Q071 | unicode-off | 1806.556 | 2141.266 | 8183.145 | 1003 | 1364 | 1364 | 10/11 | 0/11 | ok |
| Q099 | unicode-off | 5002.067 | 6490.205 | 17735.966 | 59586 | 59586 | 0 | 10/11 | 0/11 | ok |
| Q113 | unicode-off | 3031.546 | 3750.416 | 6770.291 | 951 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q114 | unicode-off | 4174.678 | 6244.541 | 11108.070 | 3977 | 4928 | 4928 | 10/11 | 0/11 | ok |
| Q148 | unicode-off | 6158.686 | 8829.867 | 20329.407 | 4105 | 4105 | 4105 | 10/11 | 0/11 | ok |

## Index construction

- "ascii": build 11.084 s; snapshot write 0.293 s; parses 148486; resident 251 MiB; peak resident 339 MiB; estimated index 65 MiB; snapshot 106 MiB; fresh-process restore min/median/max 0.493/0.502/0.549 s over 5 runs.
- "unicode": build 12.245 s; snapshot write 0.435 s; parses 148486; resident 336 MiB; peak resident 408 MiB; estimated index 77 MiB; snapshot 116 MiB; fresh-process restore min/median/max 0.509/0.516/0.559 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
