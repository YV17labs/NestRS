# A column reads in one type, and an instant in one spelling, on every edge

Written on 2026-11-18, before 7.0's first publish, when a GraphQL timestamp was
found to be two types: aligning them after it would change the SDL or the wire,
a break inside 7.x (`semver-7x.md`).

`#[expose]` rewrote a column's type by the spelling of its bare type, so an
`Option` kept the type async-graphql gives it, which nest-rs-seaorm turns on
(`uuid`, `chrono`). Seen in process, one row at `2026-10-08T12:30:00.250Z`: a
`DateTimeWithTimeZone` read as `String!`, an `Option` of one as the `DateTime`
scalar, an `#[expose(nullable)]` one as `String`, while a write's input took
`DateTime!`; a `Uuid` read as `String!`, an `Option<Uuid>` as `UUID`. And
async-graphql writes a chrono instant with `to_rfc3339()`, `+00:00` at offset
zero, where chrono's `Serialize` — what REST, a WebSocket message and an MCP
tool's answer write — says `Z`: an `Option` timestamp, a `DateTimeUtc` and a
`DateTimeLocal` read one instant two ways depending on the edge. 6.x read the
same types, and wrote the bare column `+00:00` on every edge.

**Decided:**

- **An instant is GraphQL's `DateTime`, whether its column is an `Option`, one a
  grant may withhold or neither, and whichever zone it holds** — chrono's
  `DateTime<Tz>` and sea-orm's aliases for it, read by spelling like the rest of
  the decorator. It is the scalar async-graphql gives chrono, the one a write's
  input already takes, and the name GraphQL's convention gives a timestamp (the
  GraphQL Scalars `DateTime` specification, GitHub's and Shopify's schemas).
- **An instant has one spelling on every edge: chrono's `Serialize`.** RFC 3339,
  `Z` at offset zero, any other offset as held, the fraction to the precision
  held (0, 3, 6 or 9 digits). `Z` is the standard's: RFC 9557 §2 updates RFC 3339
  §4.3 so that `Z` says the time in UTC is known and the local offset is not,
  which is what a `timestamptz` read back holds, where `+00:00` claims UTC as the
  preferred reference point. The GraphQL Scalars specification writes `Z` too,
  and zod's `z.iso.datetime()` accepts no other by default.
- **The wire field holds `nest_rs_seaorm::__private::WireDateTime`**, tier 2:
  chrono's own `Serialize` and JSON Schema, and an async-graphql scalar named
  `DateTime` that writes the same string; a value for logic is read off the
  `Model`. Its description and `@specifiedBy` URL are async-graphql's own, word
  for word, because a schema prints the first `DateTime` it registers, a row's or
  an input's: held by `nest-rs-seaorm`'s
  `the_date_time_a_row_registers_is_described_as_the_one_an_input_registers`.
- **An id reads as its `String` whether its column is an `Option` or not**, the
  type every `#[crud]` id argument takes, which parses it to a v7 key and refuses
  any other with the framework's problem on every edge alike.

**Refused:**

- **`String` on every read**, the bare column's shape: a timestamp would read
  as one scalar and be written as another, `DateTime!` on a write's input, and
  lose the type every GraphQL client generator maps.
- **async-graphql's own scalar on every read**: one type, but `+00:00` on
  GraphQL and `Z` on every serde edge; or every edge re-spelled `+00:00`, against
  RFC 9557, zod's default and the spelling an app's own serde DTOs keep.
- **The GraphQL Scalars `DateTime` verbatim**, its URL as `@specifiedBy`: it
  writes exactly three fraction digits, and Postgres keeps six, so an
  `updatedAt` read back would no longer match the row it names. The scalar is
  specified by RFC 3339, as async-graphql's is.
- **`UUID`, or `ID`, on every id**: every object's id and every `#[crud]`
  argument would change type, and a malformed id would be refused by
  async-graphql's scalar parse rather than by the problem every edge answers. A
  write's input keeps the column's type, `UUID`, parsed at the edge: that
  asymmetry is left as it was.
- **async-graphql's `derived` field over the native column**: its type is a
  string, which the decorator's paths cannot reach under the umbrella's root.
  **A field resolver per instant**: async-graphql allows one `ComplexObject` per
  type, which an entity's own `#[field_resolver]` holds.
- **A public instant type**: an item every 7.x keeps, for a field nothing reads
  but a serializer.

**Not covered:** an array column, `Vec<…>`, which sea-orm maps only under its
`postgres-array` feature, which neither the framework nor `nestrs new` turns on;
and an app's own GraphQL type holding a chrono instant, which async-graphql
spells `+00:00` (`.claude/upstream/async-graphql.md`).
