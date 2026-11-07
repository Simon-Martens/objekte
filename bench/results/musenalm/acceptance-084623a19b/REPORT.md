# Final optimized/reference acceptance

Code revision: `084623a19b`. Both runs restored the final snapshot and
compared the existing 150 Musenalm requests against the independent index-free
reference evaluator. Counts and every ordered page hit agreed, including file
metadata, byte ranges, matched-field evidence, and requested payload.

| Alphabet | Cases passed | Failures | Full log |
|---|---:|---:|---|
| ASCII | 150/150 | 0 | [ascii.log](ascii.log) |
| Unicode | 150/150 | 0 | [unicode.log](unicode.log) |

Reproduce with `target/release/examples/accept_musenalm --alphabet ascii` and
`target/release/examples/accept_musenalm --alphabet unicode` after building
that release example at the measured code revision.
