# Benchmark commands

All measurements used release builds and `examples/musenalm`.

```text
cargo build --release -p objekte-search-core --example bench_musenalm
```

Native construction, three fresh processes per alphabet:

```text
target/release/examples/bench_musenalm --incremental --cases Q001,Q119 \
  --profiles ascii-off --repetitions 1 --result-dir native-ascii-N
target/release/examples/bench_musenalm --incremental --cases Q001,Q119 \
  --profiles unicode-off --repetitions 1 --result-dir native-unicode-N
```

Archive construction was repeated with `--archive --archive-workers 1|8|32`.

The complete 150-case query matrices used:

```text
target/release/examples/bench_musenalm --incremental \
  --profiles ascii-off,unicode-off --repetitions 1
target/release/examples/bench_musenalm --incremental --archive \
  --archive-workers 8 --profiles ascii-off,unicode-off --repetitions 1
target/release/examples/bench_musenalm \
  --profiles ascii-off,unicode-off --repetitions 1
target/release/examples/bench_musenalm --archive --archive-workers 8 \
  --profiles ascii-off,unicode-off --repetitions 1
```

The progressive results in `query-summary.json` come from the final `-v3` directories. The direct results come from the final `-v2` directories.
