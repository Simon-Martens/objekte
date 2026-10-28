# Minimal test repository

A small independent fixture for the first Objekte profile. Tests copy it into a
fresh temporary directory and keep application state outside its source folders.

- `books`: nullable/missing scalar fields, fractions, booleans, opaque JSON, literal
  HTML/Markdown, punctuation in property names, and nested reference occurrences.
- `people`: references back to books, forming a collection cycle.
- `spare`: an unrelated collection for proving lazy reads.

`books/b` deliberately refers to a missing person. Null, empty, and absent relation
values produce no edges. The two occurrences of `p1` in its `links` array remain
separate edges. `d` has explicit nulls; `e` has absent title/score fields. The default
score order is `f, b, a, g, c, d, e`; descending is `c, a, g, b, f, d, e`.

The fixture is data, not library behavior. No collection or field names from this
fixture or Musenalm are hard-coded in the library.
