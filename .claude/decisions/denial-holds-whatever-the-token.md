# A denial holds at the class gate, whatever the token carries

## 2026-10-31 — a `cannot` no `when` narrows refuses outright, and takes no scope

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity
audit's second pass, its first item: car-1 and car-2), public API break
included.

Until then the class gate read grants only: `Ability::can_class` asked whether
any grant matched, and a denial — even one with no `when`, matching every row
— reached it as nothing. Under CASL's canonical `can(Manage, E).when(org)` plus
`cannot(Create, E)`, `#[crud]`'s create ran its `INSERT` for a caller the
policy denies outright: a taken unique value answered `409` across tenants, an
unfillable NOT NULL column a `500` filed at `error`, and only an insert that
landed reached the re-check's `403`; a list under `cannot(Read)` answered
`200 []`, and a hand-written `#[authorize(Create, E)]` body ran whatever it
did outside `Repo`. Beside it, a denial took `requires_scope` like a grant,
and a token without the scope dropped the denial: the narrower token got more
than the wider one, against RFC 6749 §3.3 and the scopes page's own "it
narrows, it never widens".

- **The class gate refuses a denial no `when` narrows**, keyed on the action
  or on `Manage`: `can_class` is any grant and no such denial, on every edge,
  since every posture runs the one `class_gate`. The refusal is a plain `403`
  (`reason = "class_denied"` on its `warn`), never RFC 6750's scope challenge,
  and `missing_scopes` names nothing: no scope lifts a denial. A denial a
  `when` narrows stays with the rows — the query pre-filter and the instance
  check — as a grant's condition does.
- **`Creatable` sends no insert no row could pass**: a caller whose ability
  `can_class` refuses `Create` on the entity — no grant, or a denial no `when`
  narrows — gets `RecordNotInserted` before the statement, the outcome its
  re-check would have reached had the database not refused first. A service
  creating under another posture's unit meets it there.
- **A denial is a rule of its own kind**: `cannot` returns
  `RuleSpec<'_, E, Cannot>`, `can` `RuleSpec<'_, S>` (`Can` by default), and
  `requires_scope` and `fields` exist on the grant's alone, so a scoped or a
  column-narrowed denial does not compile (two trybuild snapshots). `fields`
  goes with it for the same reason: a denial decides visibility, never
  columns, so it was a call that did nothing. The kinds are read off the
  builder's own two methods (CASL's `can` and `cannot`), and the private field
  holding a grant's columns is `columns`, so rustc's refusal of `fields` on a
  denial reads as a missing method, not a private field.

**Refused:**

- **The behaviour-only fix** (the denial materialized whatever the scopes,
  `requires_scope` kept on it): the call would still compile and mean nothing,
  a knob a reader takes for a condition.
- **`requires_scope` kept on every kind behind a sealed trait with an
  `on_unimplemented` sentence**: one more public name and a method rustdoc
  lists where it can never be called; rustc's own "the method was found for
  `RuleSpec<'_, S>`" names the grant already.
- **A separate `DenialSpec` beside `RuleSpec`**: `RuleSpec` would have named a
  grant alone, the set reading `{RuleSpec, DenialSpec}`; renaming it
  `GrantSpec` was a second break for no reader.
- **Markers named `Grant` and `Denial`**: `nest_rs::guards::Denial` is the
  refusal every guard returns, and `Predicate::Deny` the malformed relation's
  sentinel; a third meaning of the word in one import list.
- **The gate deciding a conditional denial**: it has no row, and the
  pre-filter and the instance check already decide it on one.

**Limits:**

- A denial whose `when` holds for every row by construction
  (`p.all([])`) is a conditional denial to the gate: the rows refuse it, the
  gate does not.
- `cannot(Read, E)` beside `can(Manage, E)` refuses a `Read` site, not a
  `Manage` one: a check of `Manage` reads `Manage`'s rules alone, as
  `condition_for(Manage)` does.

## 2026-10-31 — a `when` holding for every row narrows nothing

Agreed by the owner on 2026-10-31 ("Tout faire (Recommandé)", the identity
audit's second pass; the item closes the first Limit above). A rule whose
`when` is `TRUE` for every row by its shape — `p.all([])`, `Predicate::Always`,
a disjunction holding one, the negation of an empty disjunction — is committed
as the rule no `when` narrows: the builder drops it at commit, and its SQL
renders `TRUE` wherever it sits inside a larger predicate, so the class gate
reads such a denial as the outright one it is, on every edge. The reading is
sound, not complete: a shape whose truth depends on a column, or on `NULL`'s
third value, stays conditional and the rows decide it. An intersection of two
rules no `when` narrows is one such rule too, its condition a single `TRUE`.

The same bar stands before `Repo`'s saves and `Creatable`'s create, for a write
no class gate saw (a handler gated on another subject): a caller no row could
pass for — the ability bars the action on every row, or a request holds no
ability, as on a route gated on a `#[subject]`, whose ability `UnmaskedReply`
withholds — gets what a row outside its scope answers, `RecordNotUpdated` or
`RecordNotInserted`, before the entity's `before_save` runs or a statement is
sent. A job, which holds none, stays unscoped, and `ability_scope`'s `warn` stays
the one place the missing ability shows. `Repo::delete` runs no hook, and its
statement carries the denial in its `WHERE`, so it still sends one:
`DeleteResult` is `#[non_exhaustive]`, and no empty result is built without the
round trip.

**Refused:**

- **Deciding a `when` by evaluating it on a synthetic row**: a column's value
  decides a comparison, and no row stands for every row.
- **Reading `col IN ()` as false for every row**: true of sea-query's
  rendering today, but a narrowing read off a renderer's spelling turns a
  grant into a rule for every row the day the spelling changes; a grant is
  never widened on a guess.

## 2026-11-05 — a site gated on `Manage` means every action

Agreed in the owner's mandate of 2026-11-05 ("on ne veut pas d'angle mort"), the
reading decided by the first value. It closes the second Limit of the first entry
above: `cannot(Read, E)` beside `can(Manage, E)` passed `#[authorize(Manage, E)]`,
so a caller the policy bars from reading a thing managed it, and four separate
grants, which open every action, were refused it.

A **grant** on `Manage` stays the wildcard, read under each concrete action's key.
A **site** gated on `Manage` — a posture, or any read the ability is asked with
`Action::Manage` — means `Read`, `Create`, `Update` and `Delete` all at once:

- **`can_class` is the AND of the four**, and `denied_outright` the OR, so a denial
  no `when` narrows on any of them refuses it (`reason = "class_denied"`), an action
  no grant opens refuses it (`no_class_grant`), and four separate grants pass it.
- **`condition_for` is the AND of the four conditions**, reduced to one when no rule
  names a concrete action: every action then reads the wildcard's rules alike, and
  the statement renders as it did. **`can` likewise**, on the model.
- **`pinned_values` keeps what any of the four pins**, the row having to fall in
  scope for each; two actions pinning a column apart leave it to the re-check.
- **A reply reads `Read`**: `permitted_fields`, `mask`, `mask_many` and every edge's
  mask judge the row at `Read`, which is the question a reply asks. A write reads the
  action it runs, never the site's, so a body under `Manage` writes under `Update`.
- **`missing_scopes` answers what lifts the site**: what each refused action lacks,
  once each, or the wildcard's own withheld grant, which opens all four, when it asks
  for fewer scopes; nothing when a denial bars one, or no grant, withheld or held,
  would open one, since a wider token would be refused all the same. An intersection
  keeps a withheld wildcard whole where the other side grants all four, and on each
  action it grants otherwise, after that action's own.
- **A `#[subject]` follows the same reading**: it takes no `cannot`, so what changes
  there is that four separate grants pass.

**Refused:**

- **CASL's reading**, where a check of `manage` merges `manage` rules alone
  (`packages/casl-ability/src/RuleIndex.ts`, read at its lookup stage): it is the shape
  this entry removes. Stricter than CASL on purpose: an ability read off a rule set
  should not answer yes to "may do everything" while it refuses one thing by name.
- **Compile-refusing `Manage` at a site**: `Ability::can_class(Action)` is a runtime
  API anyway, and `#[authorize(Manage, Transcoding)]` is how a capability reads.
- **`Manage` at a site reading as "any action"**: it would widen what a posture
  demands past what its name says.

**Limits:**

- `missing_scopes` at a site gated on `Manage` weighs two sets, each refused action's
  first grant and the wildcard's. Each suffices, but the one named is not always the
  smallest when one action's second grant and another's first share a scope: finding
  the smallest is a set cover no refusal needs. It names a little more than needed,
  never less.
- A reply's row under a site gated on `Manage` is trusted as `Loaded` for `Read`'s
  rules, whichever action the body read it for, as a reply under any other posture
  trusts its read (`mask-provenance.md`).
