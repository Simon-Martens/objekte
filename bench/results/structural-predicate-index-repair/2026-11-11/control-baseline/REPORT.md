# Musenalm query checkpoint

Code revision: `95f1736d448a01d94650afd237149c158c0e7a8a`. Repetitions: 7. Failures: 0.

| Case | Profile | Min µs | Median µs | Max µs | Matches | Candidates | Parses | Prepared hits | Verified hits | Status |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Q003 | ascii-off | 16743.549 | 17231.756 | 18246.000 | 2866 | 2948 | 2948 | 6/7 | 0/7 | ok |
| Q072 | ascii-off | 9389.821 | 9976.342 | 13983.607 | 1287 | 2251 | 2251 | 6/7 | 0/7 | ok |
| Q073 | ascii-off | 2584.653 | 2646.188 | 3276.342 | 225 | 893 | 893 | 6/7 | 0/7 | ok |
| Q076 | ascii-off | 32528.817 | 33900.804 | 51637.859 | 2206 | 12554 | 12554 | 6/7 | 0/7 | ok |
| Q119 | ascii-off | 25483.489 | 33289.857 | 46724.193 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | ascii-off | 7610.611 | 7900.204 | 8488.669 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q144 | ascii-off | 40301.413 | 46399.884 | 63566.867 | 3939 | 8570 | 8570 | 6/7 | 0/7 | ok |
| Q159 | ascii-off | 17431.400 | 20594.079 | 24199.298 | 5 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q174 | ascii-off | 44879.329 | 54937.535 | 70702.406 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q177 | ascii-off | 16949.716 | 23022.539 | 34823.657 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q180 | ascii-off | 2949.879 | 3106.765 | 3945.318 | 18 | 1202 | 1269 | 6/7 | 0/7 | ok |
| Q187 | ascii-off | 449831.264 | 457940.992 | 550995.708 | 133948 | 133948 | 133948 | 6/7 | 0/7 | ok |
| Q190 | ascii-off | 19038.979 | 19585.064 | 30369.593 | 3821 | 3821 | 3821 | 6/7 | 0/7 | ok |
| Q003 | unicode-off | 17106.560 | 17498.667 | 19074.837 | 2866 | 2948 | 2948 | 6/7 | 0/7 | ok |
| Q072 | unicode-off | 8978.870 | 10050.592 | 13170.449 | 1287 | 2251 | 2251 | 6/7 | 0/7 | ok |
| Q073 | unicode-off | 2438.579 | 2482.771 | 2805.948 | 225 | 893 | 893 | 6/7 | 0/7 | ok |
| Q076 | unicode-off | 32567.141 | 34414.469 | 46015.632 | 2206 | 12554 | 12554 | 6/7 | 0/7 | ok |
| Q119 | unicode-off | 25450.437 | 34657.045 | 40258.633 | 133948 | 133948 | 0 | 6/7 | 0/7 | ok |
| Q141 | unicode-off | 7656.266 | 7883.222 | 11233.232 | 823 | 982 | 982 | 6/7 | 0/7 | ok |
| Q144 | unicode-off | 38212.440 | 44994.495 | 57367.087 | 3939 | 8570 | 8570 | 6/7 | 0/7 | ok |
| Q159 | unicode-off | 15850.743 | 16324.201 | 23677.338 | 5 | 4928 | 4928 | 6/7 | 0/7 | ok |
| Q174 | unicode-off | 45019.332 | 53241.480 | 69687.840 | 4296 | 9398 | 9398 | 6/7 | 0/7 | ok |
| Q177 | unicode-off | 16514.048 | 17597.132 | 32866.242 | 86564 | 86564 | 0 | 6/7 | 0/7 | ok |
| Q180 | unicode-off | 3058.032 | 3696.681 | 4250.141 | 18 | 1202 | 1269 | 6/7 | 0/7 | ok |
| Q187 | unicode-off | 448729.385 | 459856.929 | 491586.697 | 133948 | 133948 | 133948 | 6/7 | 0/7 | ok |
| Q190 | unicode-off | 19362.226 | 20966.908 | 32321.529 | 3821 | 3821 | 3821 | 6/7 | 0/7 | ok |

## Index construction

- "ascii": build 2.480 s; snapshot write 0.431 s; parses 148486; resident 529 MiB; peak resident 555 MiB; estimated index 65 MiB; snapshot 121 MiB; fresh-process restore min/median/max 1.349/1.362/1.409 s over 5 runs.
- "unicode": build 2.514 s; snapshot write 0.437 s; parses 148486; resident 640 MiB; peak resident 640 MiB; estimated index 68 MiB; snapshot 123 MiB; fresh-process restore min/median/max 1.340/1.349/1.451 s over 5 runs.

Timing includes COUNT and requested page/payload. Diagnostic plans are captured outside primary samples.
