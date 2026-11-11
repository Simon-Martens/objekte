# Musenalm query checkpoint

Code revision: `6ef66ec8f4bf15bfcaaf6cc4c7493bcf9bb592fc`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q035 | ascii-off | 52.098 | 55.013 | 169.067 | 124 | 124 | 0 | 6/7 | 0/7 | ok |
| Q056 | ascii-off | 1355.757 | 1359.865 | 3925.395 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q115 | ascii-off | 106.239 | 109.135 | 193.764 | 244 | 244 | 0 | 6/7 | 0/7 | ok |
| Q120 | ascii-off | 3760.986 | 5819.483 | 8835.729 | 42700 | 42700 | 0 | 6/7 | 0/7 | ok |
| Q141 | ascii-off | 7285.818 | 8210.024 | 25126.796 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q150 | ascii-off | 7962.339 | 10430.506 | 17749.395 | 390 | 457 | 457 | 6/7 | 0/7 | ok |
| Q160 | ascii-off | 341659.673 | 344767.542 | 355495.697 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q175 | ascii-off | 9050.904 | 9304.610 | 9689.443 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q189 | ascii-off | 1090.158 | 1126.817 | 2119.341 | 7 | 203 | 203 | 6/7 | 0/7 | ok |
| Q190 | ascii-off | 23844.306 | 26967.343 | 65316.991 | 3821 | 3821 | 3821 | 6/7 | 0/7 | ok |
| Q035 | unicode-off | 52.639 | 54.222 | 172.264 | 124 | 124 | 0 | 6/7 | 0/7 | ok |
| Q056 | unicode-off | 1361.938 | 1385.342 | 3881.763 | 11672 | 11672 | 0 | 6/7 | 0/7 | ok |
| Q115 | unicode-off | 105.118 | 106.420 | 197.351 | 244 | 244 | 0 | 6/7 | 0/7 | ok |
| Q120 | unicode-off | 4204.930 | 6702.662 | 8075.011 | 42700 | 42700 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 7145.103 | 8736.584 | 20334.272 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q150 | unicode-off | 7150.514 | 9394.690 | 16070.572 | 390 | 457 | 457 | 6/7 | 0/7 | ok |
| Q160 | unicode-off | 327128.673 | 333682.065 | 349395.235 | 70386 | 70390 | 70390 | 6/7 | 0/7 | ok |
| Q175 | unicode-off | 9019.434 | 9338.194 | 9764.344 | 95153 | 95153 | 0 | 6/7 | 0/7 | ok |
| Q189 | unicode-off | 967.498 | 1043.370 | 1461.225 | 7 | 203 | 203 | 6/7 | 0/7 | ok |
| Q190 | unicode-off | 19515.253 | 33108.660 | 55854.653 | 3821 | 3821 | 3821 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.689 s; snapshot write 0.554 s; parses 148486; resident 480 MiB; peak resident 543 MiB; estimated index 66 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.336/1.344/1.489 s over 5 runs.
- "unicode": build 2.636 s; snapshot write 0.567 s; parses 148486; resident 739 MiB; peak resident 739 MiB; estimated index 69 MiB; snapshot 125 MiB; fresh-process restore min/median/max 1.347/1.355/1.555 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
