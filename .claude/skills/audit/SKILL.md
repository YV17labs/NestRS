---
name: audit
description: Adversarially audit the working tree, or a scope named as an argument. Agents prove defects and never fix them; silence outranks noise. Use before calling a change done, and after fixing anything an earlier audit found.
---

# `/audit` — find what green tests missed

Passing tests are not evidence. A test finds what its author thought of, so a
suite written alongside a change is blind in exactly the same places the change
is. This skill exists because that blindness has produced shipped defects here
three times, twice in code written *while fixing* the previous round.

**Run it before calling any non-trivial change done, and again after fixing what
it finds** — the fix is new code, reviewed by nobody.

## How to run it

1. **Scope it.** The argument, if given, is the scope (a file, a crate, a
   subsystem). With no argument, use the working tree: `git status --short` and
   `git diff`, plus untracked files, which are usually where the new work is.

2. **Split into narrow lanes — never one agent for everything.** An agent given
   forty things to check does all forty shallowly. An agent told "attack this one
   file" finds real defects. Aim for **two to five lanes**, each a file or a
   coherent subsystem, each with its own list of what to try breaking.

3. **Give every lane the mandate below, verbatim in substance.** The last two
   paragraphs are the ones that pay; without them an agent returns a reassuring
   report.

4. **Triage yourself.** Agents do not fix. You decide what is real, what is
   already covered, and what is an owner decision. Then fix, add the regression
   test, and run the *Definition of done* for every workspace touched.

5. **Report to the owner**: what was found, what you fixed, what you did not and
   why, and — separately — which areas came back clean versus which were never
   attacked. Those are different facts and the owner needs both.

## The mandate each lane gets

> Audit `<scope>`. **Prove, do not fix.** For each finding: the probe you
> actually ran and its real output, pasted. Something you could not reproduce is
> reported as "suspected, could not reproduce" — never as fact. An invented
> finding is worse than an empty report.
>
> Rank by **silence first**: something that serves a wrong answer without saying
> so, then something that fails loudly but wrongly, then performance, then
> ergonomics. A defect that announces itself is a smaller defect.
>
> Delete every probe before you finish and say so. Scope every cargo invocation
> with `-p`; another process may be compiling in the same target directory.
>
> Finish by naming the areas you attacked and found **clean**, and separately the
> areas you **did not get to**. I need to tell "clean" from "not looked at".

## What to hunt — the classes that have actually bitten

Each line below is a shipped defect from this repo, generalised. Hunt these
before hunting anything generic.

- **A shadow implementation.** Two pieces of code answering one question, only
  one of which is the authority. A path matcher beside the router; a casing rule
  in two crates; a schema check beside serde. It drifts, and the drift is silent
  because both halves look right in isolation. *Ask: who owns this answer, and is
  this code asking them or guessing?*
- **A value where an error is honest.** `Ok(None)`, `[]`, `false`, a default —
  returned on a path that decides access, routing or identity. *Ask: is this
  answering, or is it giving up quietly?*
- **A check that only runs in some compositions.** A boot validation living in an
  optional module, so an app that does not import it is unguarded. *Ask: does
  this check belong to the crate that owns the thing it validates?*
- **Prose the code contradicts.** A doc comment or a docs page stating behaviour
  that was true once. *Ask: did I verify this sentence, or inherit it?*
- **A decorator argument silently ignored.** An unparsed key, a flag consumed and
  dropped. *Ask: what does the developer see if they write this and it does
  nothing?*
- **A loose match that is not obviously safe in both directions.** Ask which
  direction of error costs a loud failure and which costs a wrong answer, and
  whether the code is loose in the survivable one.
- **A per-request cost on a path that can never change an outcome** — a layer
  installed on configuration alone rather than on whether it has work to do.

## Safety — learned the hard way

- **Never `git checkout`, `git restore`, `git stash` or `git clean`.** An audit
  runs against uncommitted work; one of these destroyed 800 lines of it here.
  Remove a probe by editing it out, exactly as you added it.
- Never fix during an audit. A fix invalidates the audit that found it and hides
  which finding was real.
- Never `--update-baseline` on the docs linter, and never weaken an assertion to
  make a suite pass.

## When to stop

Track what each round returns. Rounds that keep surfacing **silent** defects mean
the component has not converged, and a fourth patch is the wrong move — say so
and put the design question to the owner. A round returning only ergonomics is
the signal that it has.
