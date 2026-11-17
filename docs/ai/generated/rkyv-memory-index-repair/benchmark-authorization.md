# Reduced benchmark authorization

On 2026-11-16 the user explicitly interrupted the full repeated benchmark run
and requested a faster benchmark with less data. The repaired convenience
matrix therefore uses:

- three fresh-process repetitions for native, reviewed-prototype, and repaired
  ASCII construction;
- one fresh-process repetition for repaired Unicode construction;
- one revision-correct repetition for each 150-case direct and progressive
  query path;
- one small mutation benchmark per revision;
- a focused three-repetition retry of the cases initially above the individual
  query investigation threshold.

The full correctness and deterministic lifecycle matrix is not reduced. This
authorization applies only to performance evidence.
