#!/usr/bin/env bash
set -euo pipefail
results=/home/simon/source/objekte-inference-snapshot-performance/bench/results/musenalm/inference-snapshot-performance-20261107
root=/home/simon/source/objekte/examples/musenalm
old_binary=/home/simon/source/objekte-snapshot-historical/target/release/examples/bench_snapshot_save
new_binary=/home/simon/source/objekte-inference-snapshot-performance/target/release/examples/bench_snapshot_save
run_one() {
    local name=$1 sample=$2 binary
    if [[ $name == old ]]; then binary=$old_binary; else binary=$new_binary; fi
    "$binary" --root "$root" --alphabet ascii --output "$results/$name.snapshot" > "$results/save-$name-$sample.json" 2> "$results/save-$name-$sample.stderr"
}
run_one old warm
run_one new warm
for pair in 1 2 3 4 5 6 7; do
    if (( pair % 2 == 1 )); then order=(old new); else order=(new old); fi
    for name in "${order[@]}"; do run_one "$name" "$pair"; done
done
