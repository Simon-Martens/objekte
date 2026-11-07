# FILTER grammar

```text
expression  := or
or          := and ("||" and)*
and         := unary ("&&" unary)*
unary       := "!(" expression ")" | "(" expression ")" | term
term        := field (":" modifier)* operator literal
operator    := "=" | "!=" | ">" | ">=" | "<" | "<=" | "~" | "!~"
             | "?=" | "?!=" | "?>" | "?>=" | "?<" | "?<=" | "?~" | "?!~"
literal     := quoted-string | signed-number | "true" | "false" | "null"
modifier    := "lower" | "length" | "each" | "null" | "exists" | registered-name
field       := dotted-name with optional ["quoted"] / ['quoted'] / [*] segments
```

`&&` binds more tightly than `||`. Unquoted leading `@` selects reserved
metadata; bracket-quoted names beginning with `@` remain data. Strings accept
either quote style and backslash escapes for quotes, backslash, newline, tab,
carriage return, and a literal percent wildcard marker. An unescaped `%` is
the only wildcard in FILTER `~`; no implicit case folding occurs. FTS is a
separate literal substring input and does not parse this grammar.

Binding validates field types, modifiers, and literals for every branch before
execution. The parser does not infer dates from string shapes.

For `:length`, a path may reach several arrays through an outer array. The
default comparison (and `:each`) requires every reached array length to satisfy
the operator; `?` operators require at least one. An empty present outer array
therefore satisfies a universal comparison and no existential comparison.
Missing paths satisfy neither. `?!=` means some reached length differs; it is
not the complement of `?=`. A single array is compared by its own length,
including zero.

## Sorting

An outermost `SORT BY` clause may follow a FILTER expression or stand alone:

```text
SORT BY title ASC, publication.year DESC
SORT BY contributors[0].name
SORT BY MIN(prices[*]), title
SORT BY FIRST(entry.preferred_title), @record.id
```

Each term accepts a local or finite related path, an optional registered
scalar sort binding (`body:normalized`), and `ASC` or `DESC`. `MIN`, `MAX`,
`FIRST`, and `LAST` reduce values within each source record. `FIRST` and
`LAST` use source occurrence order, including repeated references. They do
not support backlink expansion. A multi-valued or object-valued path without
an appropriate reduction produces an error. Missing and unavailable values
keep their configured placement; identity breaks equal-key ties.

The same request is available programmatically through `QueryOptions.sort`
and `SortField::computed(SortExpression { path, binding, reducer }, descending)`.
Use `SortField::simple(path, descending)` for the earlier scalar form; existing
serialized simple entries keep their meaning. Sorting an unregistered
path computes keys for every match, so a small page still reads and ranks all
matching records. The executor retains at most `offset + limit` winners and
enforces a 128 MiB combined budget for computed sort payload estimates, rank
keys, related target projections, and warm-cache grouping estimates. Configured scalar sort columns and
explicit composite orders can avoid per-record key extraction. Case or text
transformations apply before key construction and do not alter source values.
