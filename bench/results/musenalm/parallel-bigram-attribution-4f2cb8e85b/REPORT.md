# Bulk bigram construction attribution

The old single-worker builder at `bf8790634e` already includes the bitmap file-membership change. The new single-worker builder at `4f2cb8e85b` adds 64-slot gram aggregation and borrowed segment input. Both run the same Musenalm schema, registrations, 148,486 parsed records, Rust 1.99.1 release, performance governor and affinity CPUs 0–31. Three fresh processes per alphabet used disabled snapshots in alternating old–new pairs. All builds had zero failed parses; schema digest and registration summaries matched.

| Alphabet | Old wall median ms | Bulk wall median ms | Difference ms | Old range ms | Bulk range ms | Old extraction/index ms | Bulk extraction/index ms | Old→bulk peak RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| ascii | 8241.963 | 5455.706 | -2786.257 | 8201.124–8248.131 | 5413.208–5480.294 | 7866.212 | 5075.332 | 237.7→237.7 |
| unicode | 8312.993 | 6208.420 | -2104.573 | 8305.793–8333.457 | 6193.517–6298.016 | 7922.896 | 5827.198 | 243.7→243.1 |

A separate 10,000-record, two-segment synthetic fixture compared the old convenience constructor with the bulk constructor in the same committed code. Five uninstrumented release runs per profile measured constructor time. Valgrind Memcheck counted whole-process heap calls and bytes, including identical fixture setup and test-harness work; their differences isolate construction allocations. Those instrumented runs are allocation diagnostics, not primary timing samples.

| Alphabet | Constructor | Median ms | Heap allocations | Bytes allocated |
| --- | --- | ---: | ---: | ---: |
| ascii | convenience | 15.749 | 135,458 | 11,019,193 |
| ascii | bulk | 8.778 | 71,470 | 3,373,570 |
| unicode | convenience | 23.833 | 143,724 | 13,263,563 |
| unicode | bulk | 13.126 | 71,734 | 3,619,932 |

The separate full four-configuration build run at `parallel-bigram-4f2cb8e85b` includes both membership and bigram changes versus the pre-membership casefold baseline; it is not used to attribute all savings to bigrams. The final parallel-build report will use these serial measurements as controls.
