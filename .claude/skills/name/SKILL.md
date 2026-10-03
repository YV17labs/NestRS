---
name: name
description: Choose a name that leaves its crate or reaches an operator — a public type, module or crate; an env var or config key; a span target or unit; a datastore key; a CLI command or flag; a public error variant. Produces the set the name belongs to, sorted, and the rule that places the next member — never a bare word. Run it before the name is written.
---

# `/name` — a name is a coordinate, never a label

Every other name follows rustc's naming lints and the path law of
`.claude/rules/architecture.md`, which this procedure never overrides. **You
cannot check a name, only a series**: a name judged alone passes, which is the
failure. So the unit of work is the set the name joins.

## The procedure

Each step has a test that can fail; a failure voids the steps below it.

1. **Name the set**: the name asked for plus two siblings that do not exist yet
   but plausibly will. Cannot find two? The axis is not found yet.
2. **Read the shared segment off something** — a standard's vocabulary, the
   domain's word, the path, the owning type — never invent it. Test: can someone
   who did not write the code say whether a candidate is a member?
3. **Most stable on the left**: sorting is lexicographic on the raw string, so
   segment order is the grouping (`TOKEN_EXPIRED`, `TOKEN_REVOKED`, not
   `EXPIRED_TOKEN`). No name is a raw-string prefix of an unrelated one —
   `EnvFilter` and `SCAN` match with `starts_with`.
4. **The next member lands in exactly one place.** Zero is a closed scheme, two
   are crossed axes: declare the matrix once (vendor, then port).
5. **One word per concept, one concept per word**, across every member; a rename
   that leaves its `*Setup`, `*Host`, config type or variable behind is half a
   rename.
6. **Say what it refuses.** A name that refuses nothing — `utils`, `common`,
   `manager`, a shrug `service` — names a slot and fills forever.
7. **Round-trip**: given the step-4 rule and no member, a fresh reader derives
   the name. A set that cannot be derived is a lookup table, and drifts.

## What you hand back

The set in one sentence; where its shared segment is read off; the members,
existing and coming, **sorted as the reader meets them**; the one-line rule that
places the next member; what it refuses; and the rung that holds it (a typed
constant or enum, `nestrs lint`, or review), saying whether that rung exists.

| Surface | The set is… | Read sorted in |
|---|---|---|
| public type | the siblings that fill one seam | the module index, a stack trace |
| module, crate | what one flat directory shows | `ls crates/`, `cargo tree` |
| env var, config key | everything one deployment sets | `env \| sort`, a chart's values |
| span target, unit | every filter one operator writes | a log filter, a query |
| datastore key | everything one backend holds for the app | a `SCAN`, a schema dump |
| CLI command, flag | everything `--help` prints | `--help`, completion |
| public error variant | every outcome a caller matches on | the `match`, the error docs |

A crate name, a crate family and a new edge are proposed with this deliverable
and asked: crate naming is the owner's (`CLAUDE.md`, *How we work*). Renaming is
free before the name is written and permanent once an operator greps it: two
passes yielding two schemes mean a missing fact about the domain, not a better
word.
