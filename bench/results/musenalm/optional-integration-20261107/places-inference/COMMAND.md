# Schema-free places inference probe

Executable: `target/release/examples/bench_optional_features`, release build with rustc 1.99.1. Benchmark-only source revision: `5f9e221b0f`; core revision: `ef44e42e56`. Runs were sequential on the same isolated Musenalm fixture. Each JSON file is untouched stdout, with stderr alongside it.

```sh
for mode in disabled report auto; do
  ./target/release/examples/bench_optional_features \
    --root /home/simon/source/objekte-optional-musenalm-fixture-20261107 \
    --source-path data/places --alphabet ascii --inference "$mode" \
    --snapshot-mode disabled --build-workers auto --query-workers 32 \
    --verified-cache-bytes 0 --skip-queries > "${mode}.json" 2> "${mode}.stderr.log"
done
```

The source content fingerprint is `b1f1cc391bbdce8d12a933e18b599d0e066abba1b5546f232c783242ea16a875` for all three. These are one-sample readiness measurements; they establish inference admission and one-pass parsing, not a speed ordering.
