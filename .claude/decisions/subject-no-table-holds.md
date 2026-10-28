# A capability no table holds is a subject, gated like an entity

## 2026-10-28 — `#[subject]`, a gate-only posture on every edge

`AbilityBuilder::can` took an entity only and `Action` is closed, so the
demo's "may transcode" was borrowed as `Manage` on `org::Entity` and checked by
a hand-written guard that re-implemented the class gate — first without the
scope challenge, then with one of its own — while the advertised
`audio:transcode` scope gated no rule, and the `#[sse]` refusal told
developers to write that guard. The identity audit's agreement
("Tout faire", 2026-10-28) covered the shape below.

**A capability is a subject `#[subject]` declares on a unit struct.** A rule
grants it whole with `ab.can(action, Subject)`, `requires_scope` included;
`#[authorize(Action, Subject)]` runs the same `class_gate` on every edge and
arms no mask, since the subject has no model. The edges cannot tell an entity
from a capability by spelling, so the choice is by type: `Subject` has a hidden
companion naming how a posture masks — an entity's rows, or nothing — and each
edge's mask dispatches on it; on HTTP a route gated this way reads as unmasked,
so its reads are still withheld the ability
(`unmasked-reply-withholds-the-ability.md` stands). An `#[sse]` route takes
such a posture and refuses an entity's, by the same type.

The decorator ships in its own `nest-rs-authz-macros`, re-exported by
`nest-rs-authz`, its name read off the `<crate>-macros` rule.

**Refused:**

- **A hand-written gate per capability** (the guard the demo had): a second
  class gate whose refusals, scope challenge and log line drift from the
  framework's, and a posture a grep for `#[authorize]` misses.
- **Borrowing an entity** (`Manage` on `org::Entity`): a rule written for the
  entity grants or withholds the capability by accident.
- **`#[authorize(.., unmasked)]` on HTTP for a capability**: the edge's
  refusal stands — a route's mask is armed by the posture's type, so there is
  nothing to switch off, and the subject's type now says it.
- **`when`, `fields` and `cannot` on a capability**: the first two narrow rows
  and columns it does not have; a denial with no row to narrow would reach the
  class gate, which reads grants only, so it would be fail-open — a grant not
  written is the denial. All three stay on entities, by type.
- **`#[subject]` in `nest-rs-core-macros`**: core composes on none, and its
  macros would name authz paths; nor in `nest-rs-seaorm-macros`, since a
  subject is no table.
- **A `Subject` associated type written by hand**: adding a required item to
  the public trait breaks a manual impl, and the decorator is the one way to
  declare a capability.
