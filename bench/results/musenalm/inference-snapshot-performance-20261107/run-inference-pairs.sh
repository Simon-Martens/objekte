#!/usr/bin/env bash
set -euo pipefail
results=/home/simon/source/objekte-inference-snapshot-performance/bench/results/musenalm/inference-snapshot-performance-20261107
root=$results/fixture
before_binary=/home/simon/source/objekte-inference-before-policy/target/release/examples/bench_inference_fixture
after_binary=/home/simon/source/objekte-inference-snapshot-performance/target/release/examples/bench_inference_fixture
run_one() {
    local name=$1 mode=$2 sample=$3 binary
    if [[ $name == before ]]; then binary=$before_binary; else binary=$after_binary; fi
    /usr/bin/time -v -o "$results/inference-$name-$mode-$sample.time" "$binary" --root "$root" --inference "$mode" > "$results/inference-$name-$mode-$sample.json" 2> "$results/inference-$name-$mode-$sample.stderr"
}
for mode in disabled report auto; do run_one before "$mode" warm; run_one after "$mode" warm; done
for pair in 1 2 3 4 5 6 7; do
    if (( pair % 2 == 1 )); then order=(before after); else order=(after before); fi
    for mode in disabled report auto; do
        for name in "${order[@]}"; do run_one "$name" "$mode" "$pair"; done
    done
done
