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
