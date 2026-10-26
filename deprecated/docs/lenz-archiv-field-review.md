# Lenz archive: field and UI review

Reviewed against the checked-in example data and the public archive on 22 October 2026. This review proposes future data modeling; the interface changes do not migrate or reinterpret archival source text.

The public [manuscript catalogue](https://lenz-archiv.de/verzeichnisse/handschriften/) distinguishes location, shelfmark, title, incipit, physical extent, notes and printed witnesses. Its descriptions include uncertain dates and mixed physical measurements. The [research bibliography](https://lenz-archiv.de/verzeichnisse/forschungsbibliographie/) and [performance catalogue](https://lenz-archiv.de/verzeichnisse/theaterauffuehrungen/) have different editorial units and should keep separate schemas.

## Appropriate controls now

| Collection | Evidence in the example | Interface default |
| --- | --- | --- |
| Manuscripts | 678 records; 667 shelfmarks, 670 titles, 107 incipits, 122 notes; 611 collection references | Title, shelfmark and resolved location columns; location, collection and missing/true/false status filters; absent fields behind Add field |
| Research bibliography | 2,133 records, 203 distinct years, 1,065 author strings | Numeric year range, author text filter, year/author/citation columns; newest year first |
| Posthumous editions | 79 records with integer years | Year range and citation columns |
| Independent prints | 18 records with years and facsimile assets | Year, title page and collation columns |
| Performances | 80 records, 16 work labels | Year range and work selector; year/work/entry columns |
| Translations | 34 records, eight language labels | Language selector and entry columns |
| Periodical contributions | 115 records, 26 `Werk` labels | Container selector, heading and source columns |
| Locations / collections | 41 locations, 62 collections | Resolved relations, concise display names, descriptions as Markdown |
| Pages | 22 records, HTML bodies and optional publication flags | Monaco HTML source, title/description list, publication/layout controls in the inspector when present |

`Verschollen` is defined but absent from all checked-in manuscripts. Absence must remain distinct from false. The UI adds it explicitly with an initial false value; existing records are not backfilled.

## Additional field types worth introducing

1. **Historical date or date interval.** Preserve the original expression alongside optional normalized earliest/latest dates and a certainty qualifier. Year-only, circa, inferred dates and ranges need their own representation. Do not convert them to guessed calendar days or RFC 3339 timestamps. Ordinary integer years remain appropriate for the current bibliography.
2. **Structured media attachment lists.** Each attachment can hold a managed file reference or external URL, display label, side/page designation and optional description. `Datei` currently contains Markdown, and many other files occur inside rich text or imported `assets`; a reviewed migration would make more references eligible for file-usage checks. Keep the original Markdown while reconciling these links.
3. **People and contributor relations.** Author strings should eventually reference authority records and support multiple contributors with roles. The 1,065 distinct strings are not evidence of 1,065 distinct people: initials, shared surnames, editors, institutions and anonymous attribution require editorial reconciliation.
4. **Work and container relations.** Performance `Werk` generally denotes a dramatic work. In `unselbstaendige_drucke`, the same key often denotes a periodical or anthology. These need different target collections rather than one generic work vocabulary.
5. **Bibliographic citation objects.** Optional structured title, contributors, container, publisher, place, year, volume and page interval would enable better filtering and export. Retain the existing formatted citation as authoritative display text until reviewed.
6. **Language vocabulary.** The eight translation language labels suit a controlled selector with stable codes and localized labels. Introduce codes through an explicit mapping; do not replace source labels implicitly.
7. **Physical extent.** A compound input could separate leaf/page counts, units, width and height while retaining the literal description. Double leaves, missing portions and uncertain dimensions make a single number insufficient.
8. **Typed external identifiers and URLs.** DOI, GND and external resource fields should validate their own syntax. Linking an authority record requires editorial confirmation of identity, not a name match alone.
9. **Publication destination.** Page `permalink` is currently a boolean in this example. A future URL-or-disabled field would represent both an explicit path and the special false state; changing this needs a schema revision.

## Semantics to preserve

- `Sort`, collection order and `_editor.manual_order` are different concepts. Column sorting changes the view, not any stored order.
- Placement is now parent-owned: Orte contain Sammlungen and/or Handschriften, and Sammlungen contain Handschriften. Manuscripts have no stored Ort/Sammlung fields; parents are discovered through child arrays.
- Optional booleans have three states: absent, false and true. Deletion is a draft action; undo restores exact content and presence.
- Markdown and HTML remain source text. Search strips Markdown presentation for readable excerpts, then escapes and highlights text; it never executes record HTML.
- New relational or compound types require reviewed data migrations. The present pass configures the UI around existing types and leaves source records unchanged.
