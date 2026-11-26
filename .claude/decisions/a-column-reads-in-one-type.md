# A column reads in one type, and an instant in one spelling, on every edge

Written on 2026-11-18, before 7.0's first publish, when a GraphQL timestamp was
found to be two types: aligning them after it would change the SDL or the wire,
a break inside 7.x (`semver.md`).

`#[expose]` rewrote a column's type by the spelling of its bare type, so an
`Option` kept the type async-graphql gives it, which nest-rs-seaorm turns on
(`uuid`, `chrono`). Seen in process, one row at `2026-10-08T12:30:00.250Z`: a
`DateTimeWithTimeZone` read as `String!`, an `Option` of one as the `DateTime`
scalar, an `#[expose(nullable)]` one as `String`, while a write's input took
`DateTime!`; a `Uuid` read as `String!`, an `Option<Uuid>` as `UUID`, a write's
input took `UUID!` and a `#[crud]` id argument `String!`. And async-graphql writes
a chrono instant with `to_rfc3339()`, `+00:00` at offset zero, where chrono's
`Serialize` — what REST, a WebSocket message and an MCP tool's answer write — says
`Z`: an `Option` timestamp, a `DateTimeUtc` and a `DateTimeLocal` read one instant
two ways depending on the edge. 6.x read the same types, and wrote the bare column
`+00:00` on every edge.

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
- **An id is GraphQL's `ID` at every position**: a reply's column, bare or an
  `Option`, a write's input, a `#[crud]` id argument, the argument
  `#[authorize(A, bind = S)]` reads, a relation's key. The GraphQL specification
  gives `ID` (§3.5.5) to an identifier "often used to refetch an object or as
  the key for a cache", which is what every one of them is, and GitHub's and
  Shopify's schemas type their ids so; one type makes an id read in one place a
  value a client sends to another unchanged. `ID` accepts any string, so the
  framework parses it at the edge, and one that is no UUID v7 — the framework's
  identifier contract, `nest_rs_core::UUID_V7_REQUIRED` — is refused there with
  `INVALID_ARGUMENT` in that one sentence, never quoting what was sent: as a
  problem on an argument, as the field's entry under `errors` in an input, where
  every rule an input declares is refused. REST, WebSocket and MCP carry the
  UUID's text, `format: uuid`.
- **The wire field holds `nest_rs_seaorm::__private::WireDateTime` or `WireId`**,
  tier 2: the column's own `Serialize` and JSON Schema, and an async-graphql
  scalar named `DateTime` or `ID` that writes the same text. Each reads back as
  its column's type through `AsRef` and `Into`, the conversion traits the Rust API
  Guidelines name (C-CONV-TRAITS), never a `Deref` of a newtype (C-DEREF), so a
  field resolver computing over its parent names no framework type; an input's id
  is built with `into()`. Their descriptions are async-graphql's own, word for
  word, because a schema prints the first scalar of a name it registers: held by
  `nest-rs-seaorm`'s
  `the_date_time_a_row_registers_is_described_as_the_one_an_input_registers`.
- **A column spelled through an alias of the app's own keeps async-graphql's
  mapping**, `UUID` for an id and `+00:00` for an instant at UTC, which the
  decorator's rustdoc says: the decorator reads a type by its name, as every
  decorator does where no value can tell it.

**Refused:**

- **`String` on every read**, the bare column's shape: a timestamp would read
  as one scalar and be written as another, `DateTime!` on a write's input, and
  lose the type every GraphQL client generator maps; an id would read as free
  text a generator types as such, and be written as `UUID!`.
- **async-graphql's own scalar on every read**: one type, but `+00:00` on
  GraphQL and `Z` on every serde edge; or every edge re-spelled `+00:00`, against
  RFC 9557, zod's default and the spelling an app's own serde DTOs keep.
- **The GraphQL Scalars `DateTime` verbatim**, its URL as `@specifiedBy`: it
  writes exactly three fraction digits, and Postgres keeps six, so an
  `updatedAt` read back would no longer match the row it names. The scalar is
  specified by RFC 3339, as async-graphql's is.
- **`UUID` on every id**: a custom scalar a generator maps to an untyped value,
  which async-graphql refuses itself, with no code, and which accepts any version.
- **An input's id held unparsed until a rule reads it**, so that an id that is no
  UUID at all would be refused in the framework's sentence too: the input type a
  service receives could then hold an id that is not one. A GraphQL input's
  malformed id is refused by async-graphql's own coercion of `ID`, which says
  what its type does not accept and quotes nothing; its code is the edge's
  coercion refusal's, a change of its own.
- **Refusing an alias at compile time**: a break for a shape sea-orm already asks
  a `column_type` for, when the rustdoc's sentence suffices.
- **async-graphql's `derived` field over the native column**: its type is a
  string, which the decorator's paths cannot reach under the umbrella's root.
  **A field resolver per instant**: async-graphql allows one `ComplexObject` per
  type, which an entity's own `#[field_resolver]` holds.
- **A public instant or id type**: an item every minor keeps, for a field read
  through the standard conversions.

**Not covered:** an array column, `Vec<…>`, which sea-orm maps only under its
`postgres-array` feature, which neither the framework nor `nestrs new` turns on;
and an app's own GraphQL type holding a chrono instant, which async-graphql
spells `+00:00` (`.claude/upstream/async-graphql.md`).

## 2026-11-18 — an input's id takes any UUID version, and an instant reads back as `DateTimeWithTimeZone`

The entry above held an input's id to a UUID v7, as a `#[crud]` id argument
is held. A review saw what that refused (in process, a `#[crud]` update and
create over HTTP and GraphQL): a row holding a key no framework mints — an
account an identity provider keeps, a table imported from elsewhere, a client's
`crypto.randomUUID()`, the v4 `jti` the authentication crate signs itself — could
no longer be replaced by `#[crud]`, since a replacement carries every writable
column back as it reads it, and nothing could relate a row to such a key. 6.1
took any version.

**Decided:** an input's id is `ID`, parsed as a UUID of any version, and one
that is no UUID at all is refused where the edge decodes it. The v7 stays where
the framework addresses a row it minted: a `#[crud]` id argument, the argument
`bind` reads, a route's path, an MCP resource's template. No input carries the
key the framework mints — the insert fills it — so every id an input holds names
another row, or a value another system drew; a foreign key to a row the
framework minted is a v7 already, and the database's constraint says whether it
exists.

**Refused: a UUID v7 on an input's id.** It added nothing a foreign key's
constraint does not hold, and refused every key another system drew.

The read-back is said exactly: an id reads back as its column's `Uuid`, which
`WireId` is beside — `Copy`, compared, hashed and printed as one — so a field
resolver's borrowed parent gives it up as `Uuid::from(parent.id)`; an instant
reads back as `DateTimeWithTimeZone` (`DateTime<FixedOffset>`), whatever zone
its column holds, where the entry above said "its column's type".
