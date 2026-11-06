# Explicit regex FTS

`FtsQuery.mode` selects `Literal` (the existing substring behavior) or
`Regex(RegexOptions)`. The pattern remains in `FtsQuery.literal` for request
compatibility, but regex mode interprets it as Rust `regex` syntax. An ordinary
literal containing punctuation or `/.../` never becomes a regular expression.
`case_sensitivity` remains independent of mode. Regex flags support multiline
anchors and dot matching newline; Unicode matching is enabled by default.
Inline flags supported by the Rust regex parser are accepted. Backreferences
and lookarounds are unsupported and fail during preparation.

The Musenalm CLI accepts `--regex PATTERN` in place of `--fts LITERAL`, with
`--regex-multiline`, `--regex-dotall` and `--fts-insensitive` as optional flags.
Programmatic callers set the typed mode. The fixed 150-query workload omits
`fts_mode` and continues to run literal searches.

Matching is Boolean within each reader-owned text segment. Anchors do not
cross separate fields or array elements. Selected FTS uses the registered
field representation; unscoped FTS uses the registered raw representation.
On a malformed raw-fallback record, unscoped regex runs over the established
recovery-decoded source text. A selected logical field remains unavailable on
that record. A custom `BindingOperation` must explicitly report
`regex_text_compatible()` to use its `map_text` representation; its literal
transformer is never applied to regex syntax. Built-in `lower` and named
case-fold text bindings transform the haystack before regex matching.

Compilation is bounded to 4 KiB of pattern text, 128 parser nesting levels,
8 MiB of compiled regex, and a 2 MiB DFA cache. Invalid or unsupported syntax
is a preparation error. Regexes are compiled once per bound collection request
and shared across selected fields and fallback verification. The matching
route reuses grouped source access and ordinary evidence/payload handling.
