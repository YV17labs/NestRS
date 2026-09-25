---
name: name
description: Choose a name — for anything: a variable, a type, a file, a folder, a crate, an env var, a config key, a CLI flag, a log target, an error variant, a DB column, a route, a queue, a branch, a doc page. Produces the *set* the name belongs to and the rule that places the next member, never a bare word. Run it before writing the name, not after.
---

# `/name` — a name is a coordinate, never a label

`/architecture` asks whether an existing name sits where it belongs; `/audit`
proves behaviour wrong; `/simplify` cuts shape. This skill runs **before any of
them**, at the moment a name is about to be written for the first time, and it
exists because that moment is the cheapest it will ever be to get right and the
most likely to be decided in two seconds on instinct.

**The claim it is built on: you cannot check a name, you can only check a
series.** A single name reads fine in isolation — that is the failure mode, not
the exception. `ThrottlerModule` is a perfectly good name and a defect inside
`nest-rs-redis`, because nothing in it says which backend a stack trace is
pointing at. Judged alone it passes; judged against its siblings and its
location it fails. So the unit of work here is never the word you were asked
for: it is **the set that word joins**, and the word is one line of the answer.

A name is a coordinate in a space that mostly does not exist yet. Get the space
right and every future member places itself, sorts itself, and is found by
someone who never read the code. Get it wrong and every future member is placed
by coin-flip, and the coin-flips are permanent — that is the grind nobody
budgets for, paid one rename at a time by people who did not make the choice.

## The procedure — seven steps, each with a test that can fail

Do all seven. They are ordered so that a failure at step *n* invalidates the
work below it; skipping one means you are guessing at the steps that depend on
it.

### 1. Name the set, not the thing

Write down the thing you were asked to name **plus at least two siblings that do
not exist yet but plausibly will.** Not variations of the word — genuinely other
members: the second error variant, the third config key, the next adapter, the
other edge.

> **Test.** If you cannot produce two, you have not found the axis, and what you
> are about to write is a one-off. Stop and look harder — a true one-off in a
> system of any size is rare, and believing you have one is the usual reason a
> series gets founded badly.

The output of this step is a **list**. Every step below operates on the list.

### 2. Read the shared segment off something — never invent it

The set splits into a part every member shares (the namespace) and a part that
varies (the member). **The shared part has to be read off something external
that already exists and that you did not choose**: a standard's own vocabulary,
the domain's word, the path the file sits at, the type it belongs to, the
protocol's field names.

> **Test — can someone who did not write the code answer "is this a member?"**
> *Does this standard name this thing?* is such a test. *Is this about auth?* is
> not. A shared word nobody can test is a **theme**, and a theme admits
> everything, so it stops meaning anything by its ninth member.

This is why the shared word is *found* rather than chosen, and why the search is
most of the work. A set whose membership is argued will be argued again, and
every re-argument renames every member.

### 3. Most stable on the left, most varying on the right

Order the segments general → specific. The left is what you already know when
you go looking; the right is what you are looking for.

This is not aesthetics. **Sorting is lexicographic on the raw string**, so
segment order *is* the grouping — put the differentiator on the left and the
family scatters across the listing, put it on the right and the family is a
block a reader sees without reading a single value.

```
TOKEN_EXPIRED   TOKEN_MALFORMED   TOKEN_REVOKED     ← one block, one concept
EXPIRED_TOKEN   MALFORMED_TOKEN   REVOKED_TOKEN     ← three strangers, filed under E, M and R
```

Three corollaries, all mechanical:

- **Inside the final segment, natural language wins.** `max_retries`, not
  `retries_max`. The general→specific law governs the *namespace* segments; the
  member is a phrase, and a phrase read backwards buys sorting nobody wanted.
- **Choose a representation whose sort is the meaning.** Dates are `2026-09-16`
  because that sorts chronologically; a numbered series is `step_02`, because
  `step_2` sorts before `step_10` and the listing starts lying at ten.
- **No name may be a prefix of another** unless the prefix relation is a level a
  reader can see. Accidental prefixes are matched by real tools — `EnvFilter`
  compares targets with `starts_with`, so `nest_rs::access` silently swallowed
  `nest_rs::access_graph` and an operator lost a startup diagnostic with no sign
  of it.

> **Test — the sort test.** Sort the list from step 1 as the reader will see it
> (`ls`, an env dump, an enum listing, a log filter, a docs sidebar). Do
> siblings sit adjacent? Does the structure show *without reading the values*?
> A concept split across the sorted listing means a qualifier is on the wrong
> side.

### 4. The next-one test — where does the one after these go?

Take the most plausible member that will be added six months from now and ask
where it lands. **Exactly one answer is required.**

- **Zero answers** — the scheme is already closed against a real case. It will
  be broken by the first person who needs that case, and they will break it
  under deadline.
- **Two answers** — two axes are crossed, and the second member through gets
  placed by whoever arrives first. If both axes are genuinely real, that is a
  **matrix**, and a matrix is declared as one: decide which axis is the prefix
  and which is the suffix, then apply it to **every** member with no exception
  (`RedisQueueModule`, `RedisThrottlerModule`, `SeaOrmDatabaseModule` — vendor
  then port, always).

### 5. Symmetry sweep — one scheme, one word per concept

Every member spells every axis with the same word, in the same position, at the
same level.

> **Test.** One odd member means either that member or the scheme is wrong, and
> **deciding which is the finding** — never a shrug, never "it's fine for now".
> Two words for one concept (`user` here, `account` there) and one word for two
> concepts are the same defect wearing opposite signs: both make the set have to
> be learned twice, and both let a query name the wrong half.

A rename that leaves a sibling behind is half a rename, and the half left behind
is the one a reader trips on: a type renamed without its `*Setup`, its `*Host`,
its config type and its variable is not renamed.

### 6. The admission test — what does this name refuse?

Ask what you would **refuse** to file under this name. If the answer is
"nothing", the word names a *slot* or an *audience* rather than a subject —
`utils`, `helpers`, `common`, `shared`, `misc`, `types`, `data`, `info`,
`manager`, `handler`, `service` used as a shrug — and a name with no admission
test fills forever, because nothing can ever be argued out of it.

> The tell is that it reads perfectly well on its own. `shared` is not a bad
> word; it names *who reaches for a thing* instead of *what the thing is*, so it
> has no members, only arrivals.

A positional word survives only on a test: `core` is legitimate exactly when
*everything composes on it and it composes on nothing* — which is checkable. A
`core` that fails that is a `shared` in a better coat.

### 7. Round-trip — derive it, don't memorise it

From the name a reader must find the thing; from the thing a reader must
reconstruct the name **without looking it up**.

> **Test.** Give the set to someone (or to a fresh agent) with the rule from
> step 4 and none of the members, and ask them to produce a member's name. If
> they cannot derive it, the set is a lookup table, and lookup tables drift.

Derivable beats short. A name that is unambiguous in a log at three in the
morning outranks a name that is short in an import, every time — a name appears
once in a declaration and forever in output nobody can add context to.

## What the skill hands back

**Never answer a naming question with a word.** The deliverable is:

1. **The set** — what it is a set of, in one sentence.
2. **Where its shared segment is read off from** — the standard, the path, the
   domain word. If the answer is "I chose it", say so; that is a finding.
3. **The members** — those that exist today, plus the two or more that do not.
4. **That list, sorted**, exactly as the reader will meet it.
5. **The placement rule** — one line a future contributor applies to add the
   next member without asking anyone.
6. **What the rule refuses** — at least one thing that does *not* go here.
7. **The mechanical half** — is any of this checkable by a linter or a
   conformance test, and does that check exist? If it is checkable and unchecked,
   say so; the rule will drift otherwise.

The name that was asked for is line 3. Handing over lines 1, 2 and 5 is what
makes lines 3 and 4 survive the next contributor.

## Every surface is a namespace — the axis is rarely the obvious one

The procedure is the same everywhere; only the reader and the sorted listing
change. The trap is naming a member against the wrong set — files against their
folder when they will be read in a search result, columns against their table
when they will be read in a join.

| Surface | The set is… | Where it is read sorted |
|---|---|---|
| variable, field | the other values of that thing, in that scope | the struct, autocomplete |
| type | the siblings that fill the same seam | the module index, a stack trace |
| file, folder | the roles the folder holds | `ls`, the file tree, a search result |
| crate, package | what a workspace's flat directory shows | `ls crates/`, a dependency tree |
| env var, config key | everything one deployment sets | `env \| sort`, a chart's values file |
| CLI command, flag | everything `--help` prints | `--help`, shell completion |
| log target, event name | every filter one operator writes | a log filter string, a query |
| metric | every series one dashboard groups | the metric explorer's autocomplete |
| error variant | every outcome a caller matches on | the `match`, the error docs |
| DB table, column, index | the columns of a join, not of one table | a schema dump, a query plan |
| migration file | the ordered history | the directory, in filename order |
| route, queue, topic | every path one gateway mounts | the route table, a broker's listing |
| feature flag | everything one build enables | the manifest's feature matrix |
| test name | the failures one run prints | the runner's output |
| branch, commit scope | the project's history | `git branch`, a changelog |
| docs page, anchor | the sidebar | the sidebar, a search index |
| JSON / wire field | the other fields of that message | the payload, a consumer's parser |

## The failure classes — each one has actually shipped

Hunt these before hunting anything generic. They are ordered by how well they
hide.

- **The one-off that reads fine alone.** A name judged in isolation, wrong
  against its location or its siblings. Invisible from the file it is declared
  in, obvious from anywhere else. *Ask: what does this look like in output that
  carries no path?*
- **The theme prefix.** A shared word chosen for a reading convenience rather
  than read off anything, so membership is arguable. It was considered here for
  `authn-*` / `authz-*` and retired on exactly this test: one candidate in five
  classified without argument.
- **The marker that distinguishes nothing.** A prefix added to separate a name
  from a collision that mostly does not happen — it was right for three names
  and noise for fourteen. A marker with no subject buys no identification, only
  length.
- **The resource word that names neither side.** `DATABASE_URL` names neither
  the crate that reads it nor the type that parses it, which is why it is
  universal *and* wrong: from the variable you cannot find the code.
- **Two vocabularies for one thing.** The span said `http.request` while the log
  line said `request served`, so the set had to be learned twice and a query
  could name the wrong one. Fixed by sharing a constant, not by choosing better
  words.
- **The half rename.** See step 5.
- **The slot folder.** See step 6.
- **The accidental prefix.** See step 3.

## In this repository

The model is already written and this skill does not restate it: **the naming
levels, crate types, role tables, folder law, precedence and reserved
vocabulary live in `.claude/rules/architecture.md`**, which is loaded in every
session, and `CLAUDE.md`'s *Naming is the pillar* is the law above it. Read them
as the answer; read this skill as the method for the cases they do not already
decide.

Three bindings are worth stating because they change what you hand back here:

- **The stem is the path.** A module's type name is the crate subject plus every
  folder below `src/`, joined — so steps 2 and 7 are frequently already answered,
  and a proposal that contradicts the path is wrong however well it reads.
- **The mechanical half is executed**, not trusted: `naming.rs` in
  `nest-rs-conformance` derives every `module.rs`, every edge adapter and every
  `#[config]` namespace in both workspaces and fails on a name that does not
  match its path; `nestrs lint` runs the same code over a scaffolded project.
  Line 7 of the deliverable means checking whether your set falls under it.
- **Some names are the owner's.** A crate name, a crate family, a new edge in
  the closed edge vocabulary, and anything on `CLAUDE.md`'s *stop and ask* list
  are proposed with the full deliverable and **asked**, not applied — a published
  name is the one thing here that cannot be cheaply undone.

## When to stop

**Renaming is free before the name is written, cheap before it ships, and
permanent after it is published** — in a log an operator greps, an env var a
chart sets, a column a query names, a crate on a registry. Spend the time in the
first window; it is the only one where the cost is a paragraph of thought.

If two passes of the procedure produce two schemes you cannot choose between,
you are missing a fact about the domain, not a better word. Name the fact you
are missing and go find it — or, if it is a decision rather than a fact, put it
to the owner as one. A name chosen to end the deliberation is the name everyone
lives with.
