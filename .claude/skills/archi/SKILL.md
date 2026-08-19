---
name: archi
description: Architect-grade review of a named scope (a directory, a crate, a subsystem) — responsibility placement, layering, naming and namespace hierarchy, conformance to the repo's written rules and to public standards (W3C, RFCs, OWASP, OTel, …). Agents argue and report, never fix. For existing code, not the current diff.
---

# `/archi` — does this code live where it belongs?

`/audit` proves behavioral defects with probes; `/simplify` cleans the current
diff. This skill answers a third question neither asks: **is each thing in the
crate, module and file that owns its concern, under the name and shape the
rules mandate?** Its findings are rarely provable by a probe — they are argued
from a written rule or from an ownership fact — and most of them end as owner
questions, because placement moves and rule drift are on the stop-and-ask list
by design.

## What makes it work here

This repo has an unusually complete written referential: `CLAUDE.md`,
`.claude/rules/architecture.md` (naming levels, role tables, folder law), and
the zone rules in `.claude/rules/`. **A finding is strong exactly when it
cites one of those sentences or names the fact that decides ownership.** A
finding with neither is taste, and taste is noise.

**The referential also includes the public standards that cover each lane's
concern** — W3C, IETF RFCs, OAuth/OIDC, OWASP, OpenTelemetry semantic
conventions, JSON Schema, semver, the Rust API Guidelines. The reviewer often
knows a standard the author did not; naming it is part of the job. The repo's
own posture (recorded in `CLAUDE.md`) is the bar: prefer the standard, and
when deviating, state the deviation and its argument in writing — a homemade
`request_id` was retired for W3C Trace Context on exactly this reasoning, and
the three deliberate deviations from OTel's log model are each argued where
they live. A deficient standard may be declined and a better local design
adopted — but only with the deficiency named; a silent deviation is a finding.

## How to run it

1. **Scope it.** The argument is the scope — a directory, a crate, a
   subsystem. With no argument, use the crates the working tree touches.
   Refuse "the whole repo" politely: propose a split into successive scoped
   runs instead, because a lane given everything checks everything shallowly.

2. **Assemble the referential per lane.** Subagents do not auto-load zone
   rules. For each lane, attach (or name for reading) the rule files whose
   `paths:` match its files — `framework.md` for `crates/nest-rs-*`,
   `features.md` for `demo/crates/features`, and so on — plus `CLAUDE.md`'s
   hard-no list and observability section, and `architecture.md` always.

3. **Split into two to five lanes, by responsibility question — not by file
   count.** Good lanes: "who owns each constant and target in this crate",
   "does the kernel know about any optional edge", "does every file match the
   role tables", "is every `pub` earned". A lane is one question asked of one
   scope.

4. **Give every lane the mandate below, verbatim in substance.**

5. **Triage yourself, into four buckets** — the bucket decides what happens
   next, so never blur them:
   - **Breach of a written rule**, rule quoted — fixable after the review, if
     the fix is unambiguous and the owner asks.
   - **Responsibility misplacement, argued** — a type, constant or check in a
     crate that does not own its concern. Owner question: the move usually has
     a layering reason the code remembers and the reader does not.
   - **Rule/code drift** — the prose says one thing, the code does another.
     Report both sides; edit **neither** (CLAUDE.md, *Autonomous work*).
   - **Best practice, no local rule** — state the practice and the cost;
     lowest rank.

6. **Report to the owner**: findings ranked by blast radius (a wrong-crate
   type outranks a wrong-module file outranks a naming nit), then — separately
   — what was examined and found conforme versus what was never examined.
   Those are different facts and the owner needs both.

## The mandate each lane gets

> Review `<scope>` as a senior architect, against the attached rules **and the
> public standards covering the concern** (W3C, IETF RFCs, OWASP, OTel
> semantic conventions, Rust API Guidelines, …) — name the standard when you
> invoke it, with the section a reader can check. **Argue, do not fix.** For
> every item you flag, give exactly one of: the rule sentence it breaches,
> quoted; the standard it silently deviates from, named; or, where neither
> decides it, the fact that decides ownership — *who owns this answer?* — with
> both placements argued in two sentences each. A finding with none of the
> three is noise; drop it.
>
> For each finding: `file:line`, what it is, where it belongs and why, and the
> blast radius — what drifts, breaks or misleads if it stays. Distinguish "the
> constant is misplaced" from "the code it names is misplaced and the constant
> merely follows it"; the second is the real finding and the first is its
> shadow.
>
> Do not fix, move or rename anything. Do not edit prose to match code or code
> to match prose — report the drift with both sides quoted.
>
> Finish by naming what you examined and found conforme, and separately what
> you did not get to. The owner needs to tell "clean" from "not looked at".

## What to hunt — the architect's classes

Each class below has already produced a real question or defect in this repo.
Hunt these before anything generic.

- **A concern in a crate that does not own it.** A type, constant, check or
  descriptor whose subject belongs to another crate — the kernel holding a
  descriptor for an optional edge, a transport holding a rule the kernel
  enforces. *Ask: who owns this answer, and does the layering force the
  placement or merely excuse it?* The layering reason, when real, is the
  finding's answer — name it.
- **Knowledge leaking down.** A lower layer naming, matching on, or special-
  casing something only an upper layer should know exists. Dependency
  direction on the manifest is the easy half; vocabulary direction in the
  source is where it actually leaks.
- **A homemade answer where a standard exists.** An invented identifier,
  header, envelope, id format, error shape or grammar covering ground a public
  standard already covers — trace context, problem details, OAuth flows,
  OTel attribute names, semver ranges. The precedent is the repo's own:
  `request_id` retired for W3C Trace Context, argument recorded. *Ask: does a
  standard answer this — and if we deviate, where is the deviation argued?*
  Declining a deficient standard is legitimate; doing it silently is the
  finding.
- **An interpreted string spelled as a literal.** Span targets, env names,
  unit names, queue names, error sentences — the rules make each a constant
  declared by its owner. A literal is both a typo surface and a second
  authority.
- **A name off the tables.** `*_module.rs`, an invented folder (`core/`,
  `shared/`, `types/`), a role suffix on vocabulary, a project or app name
  below its level, a `Service` that owns no domain logic.
- **A name outside its namespace or hierarchy.** A name is read *with* its
  path, and the two spell the meaning exactly once: an item restating its
  module (`target::TARGET_ACCESS_GRAPH`) is redundancy, an item ignoring it is
  an orphan the grep never finds. Siblings at one level follow one scheme —
  one odd member means either it or the scheme is wrong, and which one is the
  finding. And every level of the hierarchy (project → crate → module → file →
  item) names exactly its own level: a level skipped, restated below its
  floor, or overloaded with two meanings breaks the property the naming law
  buys — that "where is this?" and "what is this?" have the same answer.
  Where the tables are silent, this principle still binds.
- **A second way to do one thing.** A second seam, constructor, helper or
  spelling beside the sanctioned one — including a convenience wrapper that
  quietly becomes the real API.
- **Speculative surface.** A `for_root` nobody calls, a `pub` nothing outside
  uses, a generic parameter with one instantiation, an abstraction with one
  user. Visibility wider than its use is a promise nobody priced.
- **Posture and security placement.** An authn/authz decision outside a guard,
  a denial below `warn`, a path that returns a value where an error is honest.
  These overlap `/audit`'s classes deliberately: here the question is *where
  the decision lives*, there it is *whether it answers wrongly*.
- **Prose the code contradicts** — a doc comment, rules file or docs page
  stating behaviour that was true once. Report the drift; never resolve it.

A finding that turns out to be provable by a probe — a wrong answer you can
reproduce — belongs to `/audit`; hand it there rather than arguing it here.

## Safety

- **Never `git checkout`, `git restore`, `git stash` or `git clean`.** The
  review may run against uncommitted work.
- Never fix, move or rename during the review — most findings here are owner
  decisions, and an eager fix pre-empts exactly the decision the review exists
  to surface.
- Scope every cargo invocation with `-p`; another process may be compiling in
  the same target directory.

## When to stop

A scope whose rounds keep surfacing *misplacement* findings has a boundary
problem, not a hygiene problem — the crate's mandate is unclear, and the tenth
finding restates the first. Say so and put the boundary question to the owner
instead of finishing the list.
