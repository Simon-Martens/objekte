# Bounded optimization experiment results

Task: `ic-stable-index-completion-and-optimization`  
Host: Linux, AMD Ryzen 7 5700G, 16 logical CPUs / 8 cores, release build, affinity 0-15.

## O01 — insert sort keys in semantic order

- 3 alternating P=8 full-build pairs, same corpus/configuration.
- Current repair median: 3.100 s.
- Semantic-order insertion candidate median: 3.144 s.
- Change: **-1.4%**.
- Configured sort job sum in sample 3: 4.087 s current vs 4.691 s candidate.
- Largest sort job: 1.187 s current vs 1.392 s candidate.
- **Rejected and reverted.** The explicit preparatory sort did not pay for
  itself; ordinary ordered insertion is not retained.

Raw: `bench/results/ic-stable-index-completion-and-optimization/optimization-o01-o02/`
(the O01 runs are under `current-*`/`candidate-*` from the initial combined
attempt; the later O02-only run is under `optimization-o02/`).

## O02 — catalog construction and straightforward copy removal

- Dense file-id path grouping and cached high-water lookup were implemented.
- 3 paired P=8 runs: current 3.061 s vs candidate 2.990 s (+2.3%).
- Combined with the final five-pair set: current 3.052 s vs candidate 3.115 s
  (**-2%**, opposite direction), so the speed effect is inconclusive.
- Dense path grouping was **reverted** to the previously proven BTreeMap path.
- Cached `next_file_id` with checked exhaustion and last-key lookup is retained
  because it is a correctness/resource cleanup independent of the ambiguous
  timing result.

Raw: `bench/results/ic-stable-index-completion-and-optimization/optimization-o02/`
and `paired-current-candidate/`.

## O03 — stable-buffer capacity control

- `shrink_to_fit` after owned memory extraction was exposed behind an explicit
  benchmark switch.
- 3 pairs: normal 2.977 s vs shrink 3.034 s (about 2% slower).
- Peak RSS: 830.0 MB normal vs 822.2 MB shrink; no material memory change.
- Reported capacity was unchanged by the shrink request in the measured path.
- **Rejected; disabled by default.** The switch remains only as experimental
  evidence and is not used by normal source-build runs.

Raw: `bench/results/ic-stable-index-completion-and-optimization/optimization-o03/`.

## O04 — cached sort-key serialization

Not run. No CPU profile or diagnostic encode/decode/allocation count was
collected that would justify adding cached encoded bytes to every transient sort
key. No speculative implementation was retained.
