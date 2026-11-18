# async-graphql

## The chrono `DateTime` scalar writes `+00:00` where chrono's serde writes `Z`

- **Seen** — 7.2.1: `ScalarType::to_value` for `DateTime<FixedOffset>`,
  `DateTime<Utc>` and `DateTime<Local>` is `self.to_rfc3339()`
  (`src/types/external/datetime.rs`), which chrono writes with `+00:00` at
  offset zero, where chrono's `Serialize` writes `Z`.
- **Effect** — one instant reads two ways in one app: `+00:00` on GraphQL, `Z`
  on every edge serde writes (REST, WebSocket, MCP). The GraphQL Scalars
  `DateTime` specification asks for `Z` (`scalars.graphql.org/andimarek/date-time`),
  and RFC 9557 §2 reads `+00:00` as UTC being the preferred reference point,
  which a `timestamptz` read back does not know.
- **Our side** — `#[expose]` writes each instant column through
  `nest_rs_seaorm::__private::WireDateTime`
  (`crates/nest-rs-seaorm/src/wire_date_time.rs`), whose `DateTime` scalar
  writes chrono's serde spelling (`.claude/decisions/a-column-reads-in-one-type.md`).
  An app's own GraphQL type holding a chrono instant keeps async-graphql's.
- **Canary** — `nest-rs-seaorm`'s unit test
  `wire_date_time::tests::async_graphql_still_spells_offset_zero_plus_zero`.
- **Upstream** — not filed.
- **When fixed** — in nestrs 8, an `#[expose]`d instant can keep its column's
  own type on the wire and `WireDateTime` goes, if the scalar's description and
  `@specifiedBy` URL stay what they are: a wire row's field changes type, which
  an app reading it through `as_ref()` sees. About an hour, the macro's tests
  included.
- **Ticket** —

  > **chrono `DateTime` scalar: write offset zero as `Z`, as chrono's serde does**
  >
  > `to_value` for `DateTime<FixedOffset>`, `DateTime<Utc>` and
  > `DateTime<Local>` calls `to_rfc3339()`, which writes
  > `2026-10-08T12:30:00.250+00:00`, while chrono's own `Serialize` writes
  > `2026-10-08T12:30:00.250Z`. A server answering REST through serde and
  > GraphQL through async-graphql spells one instant two ways. The GraphQL
  > Scalars `DateTime` specification asks for `Z` in result coercion, and RFC
  > 9557 §2 gives `Z` the meaning a stored instant has (UTC known, local offset
  > unknown). Could `to_value` use `to_rfc3339_opts(SecondsFormat::AutoSi, true)`?
  > Input coercion already accepts both.
