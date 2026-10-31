# The mask decides only what the rebuilt row shows, and its provenance decides the rest

The response mask rebuilds a `Model` from the wire value: an unexposed column
takes a placeholder the derive invents, and a relation is not there at all. Each
rule then ran in memory as if the row were whole. A relation answered `true`, a
hidden column compared its placeholder, and every mask counted the answer as
decided. On a subscription item a relational grant admitted every post to every
subscriber; on a decorated reply a relational denial dropped every row its read
had admitted, and a relational grant with a wider field set widened the fields
of a row a narrower grant admitted. A mask is documented fail-closed, and
"No exposure outside `#[expose]`" is a hard "no" (`CLAUDE.md`).

**The row says what it can, as the rendered SQL would.** The in-memory check
computes SQL's truth on the row's own columns — `TRUE`, `FALSE`, or `UNKNOWN`
from a comparison with `NULL`, except that sea-orm renders `eq` and `ne`
against `NULL` as `IS NULL` and `IS NOT NULL`, which the row decides — and
*undecidable* for a relation, a column the fill may invent, an exposed column
the object left out (serde's `None` is not a value it sent), or two value types
only the database compares. Undecidable parts settle nothing a decided part does
not: `org = 7 OR related(…)` is decided on a row of org 7.

**Where the row came from decides what undecidable means.**

- **Loaded** — a decorated reply, read through `Repo` under this ability, whose
  SQL applied `(OR grants) AND NOT (OR denials)`. When no decided grant matches,
  an undecided one did; no denial did. Fields come from the decided grants, else
  the intersection of the undecided ones: whichever SQL matched, that much is
  certainly granted.
- **Pushed** — a subscription item or a `masked_reply` value, which no read
  filtered: an undecided grant grants nothing, an undecided denial denies, and
  the first refusal that undecidability alone made per entity and action files
  one `warn`, so a silent stream is visible without a line per item. OWASP ASVS
  5.0 V8: authorization fails securely.
- **Held** — a model the caller checks itself with `Ability::can`, read for
  this action or another, so no read is known to have applied this action's
  rules. A grant's relation is taken to hold, as `can` always took it, so its
  negation grants nothing; a grant undecided otherwise grants nothing, and an
  undecided denial denies. `Loaded` here would let a relational denial on
  `Update` pass a model read for `Read`.

Prior art: Postgres row-level security evaluates where the rows are; CASL keeps
`accessibleBy` (query) apart from `can` (instance) (**unverified** against
today's CASL docs).

**Refused:**

- **Deciding a pushed relational rule by re-reading the row through `Repo`
  under each subscriber's ability**: a round trip per item per subscriber on the
  hot path, inside a transaction the push path does not own.
- **Skipping an undecidable rule on a reply** (the first draft): no relational
  grant would count, so every row a relational grant admitted would drop.
- **A structural decidability check** — undecidable as soon as a relation or an
  invented column appears anywhere in the rule: it refuses a pushed row its own
  columns settle, the public-or-member shape every feed has.
- **Naming invented columns by serialized key** (`placeholdered() -> &[&str]`):
  a predicate holds `Column` values, which sea-orm gives no `PartialEq`, and a
  key is not a column name once `column_name` or `enum_name` moves one. The
  derive answers `is_placeholdered(column)` with a `matches!` over the variants.
- **Shipping a refused lone pushed object as `{}`** (every field stripped): it
  still tells the recipient a row exists. It comes back `null`, as a list drops
  the row; a decorated reply's lone row stays the one its read admitted.

**The limits kept:** a decorated reply whose rows did not come through `Repo`
under the same ability — a cache, a hand-built value — keeps the trust a read
earns, and `can` takes a grant's relation to hold on a model read for another
action. A push to other connections (`WsServer::emit`, `broadcast`) is masked
for the sender's ability, if at all, never per recipient; a per-recipient mask
on `WsServer` is deferred.

## 2026-10-14 — `can` grants nothing on a relation, and `Repo::can` decides it

The limit above is closed. `Held` took a grant's relation to hold, which is
true only of a read scoped to the same action, so `can(Update, &row)` on a row
read for `Read` granted `Update` to every user a relation-gated grant names
nowhere. A security check whose answer is wrong for its most natural call is
not safe by default.

- **`Held` now reads a rule as `Pushed` does**: an undecidable grant grants
  nothing and an undecidable denial denies; it files no warn, since its caller
  hears `false`. `can` stays synchronous and queries nothing. `Loaded` (the
  masks) is unchanged.
- **`Repo::can(action, &model)` is the instance check SQL decides**: one query
  on the model's primary key under `condition_for(action)`, on the ambient
  executor and ability, as every `Repo` read. It lives on `Repo` because it
  reads the database, which only `Repo` does; it carries `Ability::can`'s name
  because it answers the same question.

**Refused:**

- **A typed proof of a same-action read that `can` would accept.** `Repo::scoped`
  returns sea-orm's `Select`, which carries no marker into the rows it yields,
  and changing that return type would move every custom query. The proof that
  exists, `Authorized<A, E>` from `CrudService::access`, lives above
  `nest-rs-authz` and is already the answer: a row bound for `Update` needs no
  check. So `Repo::can` is the one spelling.
- **A flag or a second in-memory method restoring the old reading**: it would
  be the unsafe default under another name.

**The limit kept:** a decorated reply whose rows did not come through `Repo`
under the same ability keeps the trust a read earns, as stated above.

## 2026-10-31 — `#[expose]` writes the round trip, and a column it cannot rebuild does not compile

The rebuild above went through serde on the `Model`: every masked entity
wrote `model_attrs(derive(Serialize, Deserialize))`, its crate a `serde`
line, and a `Model` that serialized was accepted by `Json` and a WS reply with
its hidden columns, on a route no mask shaped. A hidden column of a type with
no automatic placeholder — a tenant `Uuid`, a timestamp — compiled, and failed
every masked reply carrying its row with a `500`, although the decorator saw
the field. The owner agreed to the change, a public API break, with the
identity audit's second pass ("Tout faire", 2026-10-31).

- **`#[expose]` emits the round trip on the entity, in `nest-rs-authz`'s
  `__private`**: each exposed column read and written through its own serde
  impls, each hidden one given a placeholder written as a Rust value, the
  invented columns and the wire keys named beside them, as before. One trait
  on the entity, so every mask takes one bound. The `Model` derives no serde.
- **A hidden column with no placeholder is refused at the field**, naming
  `#[wire_default]`, `#[wire_default(expr)]` and `#[expose]`; a hidden primary
  key is refused naming `#[expose]`, since a key is never invented.
- **A hidden column is never read off the wire**: its placeholder stands,
  whatever an object carries under its key, which a rule never decided on
  anyway.
- **The mask cuts the wire row** to its exposed, granted keys in place rather
  than writing the rebuilt model again, and `Ability::mask` writes the exposed
  columns alone — before, an unrestricted grant handed a hidden column out of
  it.

The provenance reading above is unchanged.

**Refused:**

- **A `Serialize` on the `Model` emitted by `#[expose]`** (routed through the
  framework, so no manifest line): it keeps `Json(model)` compiling for every
  entity, the path this closes.
- **Skipping the rebuild when no rule of the caller's has a `when`**, keeping
  only the cut to exposed keys. Measured in release on a 100-row list of a
  user-shaped entity: the rebuild costs 23.5 µs of the shaper's 100.5 µs
  (parse and serialize alone 51.4 µs). The rebuild is also the check that the
  body is the entity's: without it, `Json<Vec<Post>>` under
  `#[authorize(Read, User)]` would ship its matching keys to an unconditional
  caller instead of failing closed, so the fail-closed check would depend on
  who calls.
- **`Default` for every hidden column**: a type without one would fail inside
  the expansion naming the trait, not the column, and an alias of a scalar
  could not be told from a domain type; the narrow set and an explicit
  `#[wire_default]` keep the sentence at the field.

**Still open, the owner's:** judging a loaded row on its `Model` before the
wire conversion, so a reply's rows are never rebuilt (this decision's
`Loaded` reading for loaded rows); and serving a field a grant may mask as
nullable on the GraphQL wire type rather than requiring an `Option` column.
