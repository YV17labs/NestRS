---
name: audit
description: Adversarially audit one change, once, when it carries risk — authn/authz, data access, persistence or transactions, concurrency or shutdown, or a published API. Scope is the diff; at most three lanes; agents prove defects and never fix them; every finding carries an impact severity (P0–P3) and the contract clause it breaks.
---

# `/audit` — find what the change's own tests missed

A suite written alongside a change finds what its author thought of. This skill
attacks the rest, against a written contract, and stops.

## When

**Once per change** that touches authn/authz, data access, persistence or
transactions, concurrency or shutdown, or a published API (`CLAUDE.md`,
*Reviews*). Not for docs, renames, refactors whose tests did not change, or
demo-only changes — CI's advisory `cargo mutants` lane already says what the
tests do not assert. Run it after `/architecture` and before `/simplify`.

## The contract a finding is judged against

The framework answers for **one** of these at a time:

- one process crash;
- one backend timeout or restart;
- a misconfiguration;
- a hostile client or payload.

A scenario needing two independent failures at once is a limit, written once
in the crate's `//!` — not a defect.

**Severity is impact**, and every finding states it with the clause above it
breaks:

| Level | Impact | Disposition |
|---|---|---|
| **P0** | security or data loss | fixed now, with a regression test |
| **P1** | a wrong answer under the contract | fixed now, with a regression test |
| **P2** | availability or performance | an issue, not a fix in this change |
| **P3** | ergonomics | dropped, unless the fix is trivial |

**Silence raises a finding within its level, never across it.** A wrong answer
nothing reports ranks above a wrong answer that errors, both at P1; it never
outranks a loud P0.

## How to run it

1. **Scope it to the diff** — `git diff <base>` plus untracked files. An
   argument may narrow it, never widen it to a crate or the repo.
2. **At most three lanes**, each one file or one coherent subsystem of the
   diff, each with its own list of what to try breaking. An agent given
   everything checks everything shallowly.
3. **Give every lane the mandate below**, verbatim in substance.
4. **Triage yourself.** Agents do not fix. Discard what has no reproduced
   probe and no contract clause; re-rank what was mis-levelled; apply the
   disposition table.
5. **Fix P0/P1**, each with the regression test that fails without the fix,
   then the *Definition of done*'s local loop.
6. **Re-audit once, only the lines a P0/P1 fix touched**, with the same
   mandate. Nothing else is audited again.
7. **Report**: each finding with its level, clause and disposition (fixed with
   which test, issue filed, dropped); and separately, what was attacked and
   found clean versus what was never attacked.

## The mandate each lane gets

> Audit `<scope>` against this contract: one process crash, one backend timeout
> or restart, a misconfiguration, or a hostile client or payload — one at a
> time. **Prove, do not fix.** For each finding give: its severity (P0 security
> or data loss, P1 wrong answer, P2 availability or performance, P3
> ergonomics), the contract clause it breaks, `file:line`, and the probe you
> actually ran with its real output, pasted. What you could not reproduce is
> "suspected, could not reproduce", never fact. An invented finding is worse
> than an empty report. A scenario needing two failures at once is not a
> finding.
>
> Delete every probe before you finish and say so. Scope every cargo
> invocation with `-p`; another process may share the target directory.
>
> Finish by naming the areas you attacked and found **clean**, and separately
> the areas you **did not get to**. I need to tell "clean" from "not looked
> at".

## Where P0 and P1 hide — hunting hints

Start here, before anything generic.

- **A hard "no" breached** (`CLAUDE.md`, *Hard "no" — security and data*): an
  authorization decision outside a guard, data access outside `Repo`, a column
  shown outside `#[expose]`, a payload value or secret in an error or log line,
  a route or tool mounted without its module imported. P0.
- **A swallowed error on a path that decides** access, routing, identity or
  persistence — `Ok(None)`, `[]`, `false`, a default returned where the call
  failed. P0 when it opens access or loses data, P1 otherwise.
- **A check that runs only in some compositions** — a validation living in an
  optional module, so an app that does not import it is unguarded.
- **A shadow implementation** — two pieces of code answering one question (a
  path matcher beside the router, a schema check beside serde), only one of
  which is the authority. *Who owns this answer, and is this code asking or
  guessing?*
- **A loose match** whose loose direction costs a wrong answer rather than a
  loud failure.
- **A decorator argument parsed and dropped**, so the developer's declaration
  silently does nothing.
- **Work that outlives its bound** at shutdown or on a backend stall: a wait the
  framework owns with no bound, or a task the way down never abandons.

## Safety

- **Never `git checkout`, `git restore`, `git stash` or `git clean`.** An audit
  runs against uncommitted work; remove a probe by editing it out exactly as you
  added it.
- Never fix during the audit: a fix invalidates the audit that found it and
  hides which finding was real.
- Never add a line to the docs lint's baseline, and never weaken an assertion,
  to make a suite pass.

## When to stop

After the one re-audit of step 6, the audit is over. **A P0 or P1 still standing
then is a design problem**, not a missing patch: stop, report it with the
evidence, and bring the redesign (`CLAUDE.md`, *Reviews*).
