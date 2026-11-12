# Focused bigram allocation experiment

Baseline `ab616dc6aa`; compact contribution candidate `ee5e1764`. DHAT used the existing ignored `bigram::tests::profile_single_worker_constructors` fixture with 10,000 records and `OBJEKTE_BIGRAM_PROFILE_MODE=bulk`. These process-wide debug-test figures include test harness and fixture setup. They are allocation evidence, not release-build timings or a measure of full-corpus peak RSS.

Build baseline with `CARGO_TARGET_DIR=/home/simon/source/objekte-bench-index-construction-data-path-baseline/target/dhat-baseline cargo test -p objekte-search-core --lib --locked --no-run`. Build candidate with `cargo test -p objekte-search-core --lib --locked bigram::tests` in its task worktree. For each alphabet, invoke the matching test executable with `OBJEKTE_BIGRAM_PROFILE_MODE=bulk OBJEKTE_BIGRAM_PROFILE_ALPHABET=<ascii|unicode> valgrind --tool=dhat --dhat-out-file=<file>.dhat --log-file=<file>.log <binary> --ignored --exact bigram::tests::profile_single_worker_constructors --nocapture`.

| Alphabet | Role | Gross allocated bytes | Allocation blocks | Peak live bytes |
| --- | --- | ---: | ---: | ---: |
| ASCII | Baseline | 3,949,429 | 71,545 | 2,513,740 |
| ASCII | Compact | 3,933,475 | 71,107 | 2,633,514 |
| Unicode | Baseline | 4,374,607 | 71,963 | 2,691,308 |
| Unicode | Compact | 4,179,837 | 71,371 | 2,817,258 |

The candidate saves 438 ASCII and 592 Unicode allocation blocks, while peak live bytes rise by 119,774 and 125,950 respectively. Both roles produced the same final key counts and estimated final index bytes within each alphabet. The `.log.gz` files decompress to the exact raw Valgrind logs; `.dhat` files are the raw DHAT profiles.
