#!/usr/bin/env bash
set -euo pipefail
results=/home/simon/source/objekte-inference-snapshot-performance/bench/results/musenalm/inference-snapshot-performance-20261107
root=/home/simon/source/objekte/examples/musenalm
before_binary=$results/bench_snapshot_save-before
after_binary=/home/simon/source/objekte-inference-snapshot-performance/target/release/examples/bench_snapshot_save
run_one() {
    local name=$1 sample=$2 binary
    if [[ $name == before ]]; then binary=$before_binary; else binary=$after_binary; fi
    /usr/bin/time -v -o "$results/codec-$name-$sample.time" "$binary" --root "$root" --alphabet ascii --output "$results/codec-$name.snapshot" > "$results/codec-$name-$sample.json" 2> "$results/codec-$name-$sample.stderr"
}
run_one before warm
run_one after warm
for pair in 1 2 3 4 5 6 7; do
    if (( pair % 2 == 1 )); then order=(before after); else order=(after before); fi
    for name in "${order[@]}"; do run_one "$name" "$pair"; done
done
