# Final independent acceptance

Code revision: `8dd0c2aa6d`. The optimized search result was compared with the
independent scanner for every fixed Q001–Q150 request under each bigram
alphabet. Each comparison checked COUNT, ordered page identity and complete
source locator metadata, matched-field evidence, and the requested byte or
logical payload. Both commands exited successfully: 150/150 ASCII and
150/150 Unicode comparisons passed. The processes restored the corresponding
snapshots before checking results.

```bash
cargo run --release --example accept_musenalm -- --alphabet ascii
cargo run --release --example accept_musenalm -- --alphabet unicode
```

The full per-case outcomes are in [ascii.log](ascii.log) and
[unicode.log](unicode.log). This is correctness evidence, separate from the
timed benchmark samples.
