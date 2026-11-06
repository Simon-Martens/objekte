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
