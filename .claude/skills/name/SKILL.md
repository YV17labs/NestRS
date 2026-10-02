---
name: name
description: Choose a name that leaves its crate or reaches an operator — a public type, module or crate; an env var or config key; a span target or unit; a datastore key; a CLI command or flag; a public error variant. Produces the set the name belongs to, sorted, and the rule that places the next member — never a bare word. Run it before the name is written.
---

# `/name` — a name is a coordinate, never a label

**Scope** (`CLAUDE.md`, *Naming is the pillar*): a public type, module or
crate; an env var or config key; a span target or unit; a datastore key; a CLI
command or flag; a public error variant. Every other name follows rustc's naming
lints and the path law, and does not need this procedure.

**You cannot check a name, only a series.** A name judged alone passes — that is
the failure mode. So the unit of work is the set the name joins, and the word
asked for is one line of the answer. The model the answer must fit — naming
levels, role tables, folder law, reserved vocabulary — is
`.claude/rules/architecture.md`; this skill is the method for what it does not
already decide.

## The procedure

Each step has a test that can fail; a failure invalidates the steps below it.

**1. Name the set, not the thing.** Write the name asked for plus **at least
two siblings that do not exist yet but plausibly will** — the next error
variant, the next config key, the next adapter. *Test:* if you cannot produce
two, you have not found the axis; look harder before founding a series badly.

**2. Read the shared segment off something — never invent it.** The part every
member shares is read off something that exists and that you did not choose: a
standard's vocabulary, the domain's word, the path, the owning type, the
protocol's field names. *Test:* can someone who did not write the code answer
"is this a member?" A word nobody can test is a theme, and a theme admits
everything.

**3. Most stable on the left, most varying on the right.** Sorting is
lexicographic on the raw string, so segment order *is* the grouping:

```
TOKEN_EXPIRED   TOKEN_MALFORMED   TOKEN_REVOKED     ← one block
EXPIRED_TOKEN   MALFORMED_TOKEN   REVOKED_TOKEN     ← filed under E, M and R
```

Inside the final segment natural language wins (`max_retries`). Choose a
representation whose sort is the meaning (`2026-09-16`, `step_02`). **No name is
a raw-string prefix of an unrelated one** — `EnvFilter` matches targets with
`starts_with`, so an accidental prefix silences a sibling. *Test:* sort the list
as the reader meets it (`env | sort`, `--help`, a log filter, an enum listing);
siblings must sit adjacent.

**4. The next one lands in exactly one place.** Take the most plausible member
six months out. Zero places means the scheme is closed against a real case; two
means two axes are crossed — declare the matrix once (vendor then port:
`RedisQueueModule`, `SeaOrmDatabaseModule`) and apply it to every member.

**5. One scheme, one word per concept.** Every member spells every axis with
the same word in the same position. One odd member means it or the scheme is
wrong, and deciding which is the finding. Two words for one concept and one
word for two concepts are the same defect. A rename that leaves its `*Setup`,
`*Host`, config type or variable behind is half a rename.

**6. The admission test — what does this name refuse?** If nothing, it names a
slot or an audience — `utils`, `common`, `shared`, `types`, `manager`, a shrug
`service` — and it fills forever.

**7. Round-trip.** From the name a reader finds the thing; from the thing they
reconstruct the name without looking it up. *Test:* give a fresh agent the rule
from step 4 and none of the members, and ask for a member's name. A set that
cannot be derived is a lookup table, and lookup tables drift. Derivable beats
short: a name is declared once and read forever in output nobody can annotate.

## What you hand back

Never a bare word:

1. **The set** — what it is a set of, in one sentence.
2. **Where the shared segment is read off** — the standard, the path, the domain
   word. "I chose it" is a finding.
3. **The members** — those that exist, plus two or more that do not.
4. **That list, sorted** as the reader meets it.
5. **The placement rule** — one line a contributor applies to add the next
   member without asking.
6. **What it refuses** — at least one thing that does not go here.
7. **The rung that holds it** (`CLAUDE.md`, *How a rule is held*) — a typed
   constant or enum, a structural check on paths, `nestrs lint`, or review.
   Say which, and whether it exists.

## Where each surface is read sorted

| Surface | The set is… | Read sorted in |
|---|---|---|
| public type | the siblings that fill one seam | the module index, a stack trace |
| module, crate | what one flat directory shows | `ls crates/`, `cargo tree` |
| env var, config key | everything one deployment sets | `env \| sort`, a chart's values |
| span target, unit | every filter one operator writes | a log filter, a query |
| datastore key | everything one backend holds for the app | a `SCAN`, a schema dump |
| CLI command, flag | everything `--help` prints | `--help`, completion |
| public error variant | every outcome a caller matches on | the `match`, the error docs |

The trap is naming against the wrong set: a type against its file when it will
be read in a stack trace with no path, a key against its module when an
operator reads it beside every other key in the store.

## Failure classes

- **The one-off that reads fine alone** — wrong against its location or
  siblings, invisible from the file that declares it. *What does it look like in
  output that carries no path?*
- **The theme prefix** — a shared word chosen for convenience, so membership is
  arguable (step 2).
- **The marker that distinguishes nothing** — a prefix added against a
  collision that mostly does not happen; it buys length, not identification.
- **The resource word that names neither side** — `DATABASE_URL` names neither
  the crate that reads it nor the type that parses it.
- **Two vocabularies for one thing** — the span and the line naming one unit
  differently. Fixed by sharing a constant, not by choosing better words.
- **The half rename** (step 5), **the slot** (step 6), **the accidental prefix**
  (step 3).

## Whose decision

A crate name, a crate family and a new edge in the closed edge vocabulary are
proposed with the full deliverable and **asked**: crate naming is locked
(`CLAUDE.md`, *Autonomous work*). Every other name in scope is decided here, by
the procedure.

## When to stop

Renaming is free before the name is written, cheap before it ships, and
permanent once an operator greps it, a chart sets it or a registry publishes
it. If two passes produce two schemes you cannot choose between, you are
missing a fact about the domain, not a better word: name the fact and go find
it.
