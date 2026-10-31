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
