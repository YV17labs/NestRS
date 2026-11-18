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

## 8.0's candidates answer `data: null` when one nullable field fails

- **Seen** — 8.0.0-rc.5: `resolve_container_inner` joins a selection's fields
  with `futures_util::future::try_join_all(fields.0).await?`, and a serial
  selection awaits each with `?` (`src/resolver_utils/container.rs:177-195`), so
  the first field error ends the container and climbs to the root. 7.2.1
  records each field's error with `ctx.add_error` and keeps the other fields
  (`src/resolver_utils/container.rs:180-208`, #1746 in 7.1.0). Over
  `Query { fine: i32, broken: Result<Option<i32>> }`, `{ fine broken }` answers
  `{"errors":[…],"data":null}` on rc.5 alone, with no nestrs code in the build;
  8.0.0-rc.3's changelog lists #1806, which reworked `data` when a result
  carries errors.
- **Effect** — it blocks the move to 8.0: on rc.5 a failing nullable field
  nulls every other field of the response, where the specification nulls that
  field alone; a mutation's fields run serially, so one already executed loses
  its answer and those after it never run. `nest-rs-graphql`'s in-process suite
  fails 35 of its 546 tests on rc.5, about 28 of them this.
- **Our side** — none: 7.x stays on `=7.2`, and 8.0 waits for nestrs 8
  (`.claude/decisions/public-dependencies.md`, 2026-11-17).
- **Canary** — none in the tree: the move to 8.0 runs `nest-rs-graphql`'s
  null-propagation suite, which fails while this holds.
- **Upstream** — not filed.
- **When fixed** — nothing in 7.x; the move to 8.0 is measured again on the
  final release, nestrs's own null propagation re-read against it.
- **Ticket** —

  > **8.0.0-rc: one failing nullable field answers `data: null`**
  >
  > With `Query { fine: i32, broken: Result<Option<i32>> }`, where `broken`
  > returns an error, `{ fine broken }` answers
  > `{"errors":[{"message":"…","path":["broken"]}],"data":null}` on
  > 8.0.0-rc.5, where the specification asks for `fine` and a `null` `broken`.
  > `resolve_container_inner` now joins the fields with `try_join_all(…).await?`
  > (and the serial path with `field.await?`), so the first error abandons its
  > siblings and propagates, where 7.1.0's #1746 ("Don't bail early on first
  > error when resolving containers") recorded it and kept them. A mutation
  > also loses the answer of a field that already ran, and the fields after it
  > never run. Could a field error be recorded on the context and the field
  > answer `null` when its type is nullable?

## The dataloader cache requires an `lru` RUSTSEC-2026-0253 names

- **Seen** — 7.2.1 and 8.0.0-rc.5 both require `lru = "0.16.2"`
  (`Cargo.toml`, `[dependencies.lru]`), and `dataloader::LruCache`'s storage
  calls `lru::LruCache::pop` (`src/dataloader/cache.rs`,
  `LruCacheImpl::remove`). RUSTSEC-2026-0253, `pop` freeing a node it still
  links when a key's `Drop` panics, is patched from lru 0.18.2.
- **Effect** — every lockfile building async-graphql's `dataloader` feature
  holds the unsound `pop`, and an app building `dataloader::LruCache`, which
  `nest_rs::graphql::async_graphql` reaches, can call it.
- **Our side** — `#[dataloader]` builds `DataLoader::new`, which caches
  nothing, so nestrs never builds the cache; `deny.toml` ignores the advisory
  with that reason, and the dataloaders page tells an app not to build it
  (`docs/src/content/docs/database/dataloaders.mdx`).
- **Canary** — `deny.toml`'s `unused-ignored-advisory = "deny"`: `just audit`
  fails once no lockfile holds an affected `lru`, and the ignore leaves.
- **Upstream** — not filed.
- **When fixed** — a 7.x release requiring `lru` 0.18.2 or later is taken in a
  nestrs minor, and the ignore and the page's caution go; a fix in 8.0 alone
  waits for nestrs 8. Minutes.
- **Ticket** —

  > **Require `lru` 0.18.2 or later (RUSTSEC-2026-0253), on 7.x too**
  >
  > async-graphql 7.2.1 and 8.0.0-rc.5 require `lru` 0.16, whose
  > `LruCache::pop` is unsound when a key's `Drop` panics (RUSTSEC-2026-0253,
  > fixed in lru 0.18.2). `dataloader::LruCache` calls `pop` from its storage's
  > `remove`, and every application enabling `dataloader` carries the advisory
  > in its audit, with no release to move to. Could the requirement move to
  > `lru` 0.18.2 or later, in a 7.2 patch as well as in 8.0?
