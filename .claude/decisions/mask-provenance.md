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
