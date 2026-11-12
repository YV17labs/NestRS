# A create fills what the grants pin, and the entity seeds the rest

`Creatable::create` and `create_from_active` fill, before the insert and its
re-check, each column the caller's `Create` grants (`Manage` included) pin by
equality and the input left unset — `Ability::pinned_values`, CASL's
`rulesToFields`. Every insert through `Repo` — `Creatable`'s and
`Repo::insert_unscoped` — then fills what no grant decides from the column
defaults `#[expose]` registers for the entity: a `Uuid` key with a v7 UUID,
and a column's `#[expose(default = …)]`. `timestamps` takes an entity with
`created_at` alone. Decided by the owner on 2026-10-28 ("Tout faire", the
identity audit's feat-1, new-6 and feat-14).

Until then a tenant-scoped create could not stay generated. The tenant was an
authorization input everywhere but on the write side: users overrode `#[crud]`'s
create on HTTP and GraphQL, posts on HTTP, each restating the `201`, the
`Location`, the posture and the validation to stamp `org_id` (and the author)
from the token, and losing the documented `409` and `Location` the generated
operation declares. The fact was already there twice — the `Create` grant pins
`org_id = actor.org_id`, and `Creatable` re-checks the fresh row against it —
so a missing stamp could only fail.

**How the fill reads the grants.** A column is filled when every grant that
constrains it pins it with `eq` to one value; a grant leaving it free admits
any value, so the one pin decides it. Two grants pinning it apart, or one
constraining it any other way (`is_in`, a comparison, a null test, inside
`any` or `not`), leave it unset, and the re-check refuses the row. A value the
input carries is never overwritten: the re-check judges it. A denial pins
nothing, and a grant withheld for its scope is no grant.

**Authorship is a `Create` grant of its own**, declared only when the subject
is a user (`actor.sub.user_id()`): `org_id = actor.org_id AND author_id =
uid`. Folding the author into the admins' org-wide grant would have narrowed
their update and delete to their own posts. The admins' org-wide grant on
posts is `Update` and `Delete`, no longer `Manage`: `Manage` includes
`Create`, so a machine admin (a client subject, which authors nothing) passed
the class gate and reached an insert with no author — a `500` where the refusal
is a plain `403`.

**The defaults are a registration, not the entity's behaviour.** `#[expose]`
submits them at link time, keyed by the entity's `ActiveModel`, the shape
`soft_delete` registers its tombstone in, and `Repo` fills them before its
insert. The entity's `ActiveModelBehavior` stays its own, hooks included, but
under `timestamps`, as before; `CreateModel` still seeds the key, so an
`ActiveModel` built from a create input carries it before any insert.

**Refused:**

- **The literal "every grant pins it" reading** of the audit's synthesis. An
  admin's org-wide grant names no author, so the author column would stay
  unset and the NOT NULL column answer a `500`.
- **`#[expose(tenant)]`, a column marker filled from `current_tenant_id()`**
  (new-6's proposal). A second source for a fact the grant already states, a
  string to parse into the column's type, and nothing for the author; the
  grant fills both, and is the check.
- **`#[expose(actor)]`, the author from the principal's subject.** A subject
  may be a client, not a user (`access-token-subject.md`); who may author is
  policy, and policy lives in the `AbilityFactory`.
- **`#[expose]` on tables no transport exposes** (a publication audit row, a
  social identity link), to seed their keys. `#[expose]` is the exposure
  audit; `rg '#\[expose'` would stop meaning what reaches the wire. Their
  migrations default the key to `uuidv7()` (Postgres 18) instead.
- **An `#[expose(default)]` on a hidden column.** It would expose the column.
  `user.role` keeps the migration's `DEFAULT 'user'`.
- **The column default applied by `CreateModel`** (feat-1's proposal). Only a
  create from the DTO would get it; `Repo`'s inserts serve every insert path,
  and fill after the grants, so a pin wins over a default.
- **The defaults in an `ActiveModelBehavior` `#[expose]` writes for every
  exposed entity** (this decision's first shape, on 2026-10-30, before the
  branch's review). It also seeded a row inserted outside `Repo`, but it took
  every exposed entity's behaviour from its author: a hand-written one beside
  it was `E0119`, so no exposed entity could keep a `before_save`, an
  `after_save` or a delete hook of its own — a public API break the owner had
  not agreed for this change. An opt-out for the hand-written impl would have
  been a second knob on one decorator.
- **`created_at` kept when an insert carries one.** The audit's synthesis said
  "when unset"; `timestamps` has always stamped every insert, and a key's
  meaning is API (the semver rule), so it still does.

**Limits:**

- A column the grants cannot decide and the database cannot default answers
  the insert's NOT NULL violation, a `500`: the rules are incomplete for that
  caller. The class gate stays optimistic, so the grant that lets a caller
  through must be one the fill can complete.
- `users`' duplicate email answers the generic `409` (`conflict`), no longer
  "a user with this email already exists".
- A row inserted outside `Repo` — the contextless path an injected
  connection serves — takes no column default: `CreateModel` seeds its key
  when it comes from a create input, and the rest falls to the migration's
  own defaults.
- A `timestamps` entity keeps no `ActiveModelBehavior` of its own, as before
  this decision: its `before_save` is the decorator's.

## 2026-10-31 — the class gate refuses a denial that holds for every row

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity
audit's second pass, car-1). The Limits line "the class gate stays
optimistic" is narrowed to conditional rules: a denial no `when` narrows
refuses at the class gate on every edge, and `Creatable` sends no insert for a
caller whose ability refuses `Create` on the class, so CASL's `can(Manage)`
plus `cannot(Create)` answers `403` before any statement, never the NOT NULL
`500` or the `409` an insert would have drawn
(`denial-holds-whatever-the-token.md`). A grant the fill cannot complete still
answers the `500`: the gate stays optimistic about conditions.

## 2026-10-31 — a unique conflict names the input's field

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity
audit's second pass, car-4). The Limits line "`users`' duplicate email answers
the generic `409`" is closed: a `#[crud]` create or update a unique key
refuses answers its `409` with the input's fields that key holds under the
`errors` member every edge already ships, in a validation failure's shape —
`{"email": [{"code": "unique"}]}` — on HTTP and in GraphQL's extensions. The
value is never quoted (`payload-never-quoted.md`): naming the column adds
nothing to what the `409` already tells about the value's existence.

**How the field is found.** `SeaOrmDatabaseModule` reads, once at boot, the
key columns of every unique index on every `#[expose]` entity's table and,
through `pg_inherits`, its partitions, since a violation reports the partition
and the partition's index — a primary key, a unique constraint (whose index
carries its name, which a violation reports), a unique index, partial ones
included — from `pg_index`, `INCLUDE` columns left out. It waits as a statement
of the pool does: for a connection, the pool's acquire budget; for the answer,
Postgres's `statement_timeout` under the settle net; past them the boot fails
naming the read. A binary with neither `http` nor `graphql` renders no `#[crud]`
write, so it sends no catalog query and cannot fail its boot on one. The edge
renders a `#[crud]` error synchronously after the unit rolled back, and Postgres
runs nothing more on the transaction a violation aborted, so the map is the
binding's, read off `current_unit_of_work`, and the request pays nothing. Each
input names the field carrying each column it sets, by the name serde reads it
under, which validation's `errors` keys share (`CreateModel::field_for`,
`UpdateModel::field_for`, which `#[expose]` writes, `None` by default for a
hand-written one); a key's columns the input does not carry are not named, so a
tenant a grant pins beside the slug the client sent names `slug`, and a key over
none of them names nothing.

**Refused:**

- **`pg_constraint` (`contype` u/p)**, the audit's proposal: the demo's own
  email key is a partial unique index over live rows, which has no constraint
  row, so the case the item exists for would have named nothing.
- **Reading the key on error**, on the pool: the edge's mapping is synchronous,
  and a round trip on the failure path of every conflicting write would be a
  query the request pays for a message.
- **Parsing the server's `DETAIL`** (`Key (email)=(…)`): it quotes the value,
  and its words follow the server's `lc_messages`.
- **A savepoint around every write**, so a conflict could be read inside the
  unit: a round trip per write on the hot path.
- **Reading on an idle connection alone** (`try_acquire`), so the boot would wait
  on nothing: sqlx hands a connection back on a task of its own after a ping, so
  a factory that read the pool just before the binding left none idle, and the
  field map stayed off for the process's whole life.
- **Registering only an entity declaring an input**: a hand-written input's
  `field_for` then had no key to answer, for one table less in one boot query.
- **Naming a field on a hand-written service's `ServiceError::Conflict`**: its
  variant holds a sentence, and carrying fields beside it changes the type
  every service matches — a break with no input to read the fields off.

**Limits:**

- A key over an expression (`lower(email)`) names nothing: its columns are an
  expression's, not the input's.
- A key created after the binding booted, a partition attached since among them,
  names nothing until the next boot, and a pool holding no connection at boot (one
  a test opened lazily, which proved no server) names nothing at all, said at
  `warn`.
- On GraphQL a two-word field is named as validation names it, `org_id`, where the
  input spells it `orgId`: one convention for `extensions.errors`, validation's.
- A hand-written service converting a unique violation through
  `ServiceError::from(DbErr)` answers the bare `409`, and so does a tool on
  MCP, which has no `#[crud]`.

## 2026-11-04 — GraphQL names a two-word field as its schema does

Agreed by the owner's "Tout faire (Recommandé)" of 2026-10-31 and "aucun angle
mort". The Limit above on `org_id` is closed, for this conflict and for validation
together: GraphQL's field unit respells every `extensions.errors` key off the
names the schema's inputs and arguments serve, so a client matches a key to the
field it sent, under whatever rename rule its input declares. HTTP keeps serde's
name, which is what its client sends. Re-casing with camelCase was refused: it
restates async-graphql's default in a second place, and answers wrong for an input
under `rename_fields`.

**The limits.** A field `#[graphql(name = "…")]` renames to another word stays a
Rust name, since the registry holds no Rust name to read it through. Inputs of one
schema spelling one word differently are told apart only when the key is itself
one of those spellings. A variable pipe answers before the schema runs, in the map
it wrote.
