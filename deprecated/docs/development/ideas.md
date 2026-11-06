# Exploratory ideas

These are possible future projects, not descriptions of current behavior or accepted implementation plans. Check the current library and benchmark evidence before adopting one.

- Expose source line and byte offsets for user-facing raw grep results. Current `--backend grep` is a candidate prefilter for record queries.
- Add further statically linked format adapters, such as Markdown with frontmatter, YAML or TOML. The current external example decodes a whole Markdown file into one logical `body` field; Markdown/HTML/XML text representations are already available for declared fields.
- Consider a Boolean syntax for standalone full-text terms if users need it. JSON filters already combine predicates with `&&`/`||` and support field-level `:fts`.
- Explore a different I/O engine only with a measured use case and parity checks. The current implementation keeps its established read and index paths.
