# Typed temporal FILTER operands

Unquoted `YYYY-MM`, `YYYY-MM-DD`, and offset-bearing timestamp values are periods. Their written precision defines a half-open range `[start, end)`. A bare year remains numeric. Quoted date and datetime operands and typed parameters retain their exact-value behavior. Date fields accept calendar periods; datetime and Unix-second fields accept calendar and timestamp periods. Time-bearing periods against Date fields fail during binding. Unix seconds use upward-rounded integer bounds, including before the epoch.

For a value `x`, `=` means `start <= x < end`, `!=` means `x < start || x >= end`, `<` means `x < start`, `<=` means `x < end`, `>` means `x >= end`, and `>=` means `x >= start`. Array quantifiers apply to each element's complete interval comparison; scalar typed postings may answer an interval exactly, while default universal array predicates retain residual verification.

Clock macros use one frozen UTC instant per search invocation. `Query::with_clock(now)` resolves them explicitly and yields a query reusable across pages. Prepared and verified cache keys include the resolved period or value. Ordinary searches resolve one current instant before preparation; snapshot restore does not freeze time.

| Macro | Value |
| --- | --- |
| `@now` | Frozen instant at nanosecond precision |
| `@yesterday`, `@tomorrow` | Frozen instant minus or plus exactly 24 hours |
| `@todayStart`, `@monthStart`, `@yearStart` | First nanosecond of the current UTC day, month, or year |
| `@todayEnd`, `@monthEnd`, `@yearEnd` | Last nanosecond before the next UTC day, month, or year |
| `@today`, `@thisMonth`, `@thisYear` | Whole UTC day, month, or year period |
| `@second`, `@minute`, `@hour` | UTC numeric components, zero based |
| `@weekday` | UTC weekday with Sunday = 0 |
| `@day`, `@month`, `@year` | UTC calendar numeric components |

Metadata fields such as `@file.modified` remain field paths. Unknown zero-argument macros fail before source records are read. Named local time zones and daylight-saving calendar macros are outside this version.

Instant macros compared with Date fields use each Date's UTC midnight as its exact instant. Their bounds are rounded upward to the next eligible midnight; a time-bearing literal still fails on a Date field. This makes `issued <= @todayEnd` include today while `issued = @todayEnd` matches no Date value.
