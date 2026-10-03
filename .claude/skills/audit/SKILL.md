---
name: audit
description: Attack one risky change with the security subagent and close the loop — prove, fix P0/P1 with a regression test, re-check once. Use on a change touching authn/authz, data access, persistence or transactions, concurrency or shutdown, or a value that can reach a reply, a log line or a stored record.
---

# `/audit` — prove what the change's own tests missed, then fix it

**When**: once per change carrying the risk above (`CLAUDE.md`, *How we work*).
Not for docs, renames, or refactors whose tests did not change.

1. **Scope it to the diff**: `git diff <base>` plus untracked files. An argument
   narrows the scope, never widens it.
2. **One or two `security` lanes**, each one coherent subsystem of the diff with
   its own list of what to try breaking. A lane given everything checks
   everything shallowly.
3. **Triage**: keep what has a reproduced probe and a contract clause; re-rank a
   mis-levelled finding.
4. **Fix** each P0 and P1 with the regression test that fails without the fix —
   `qa` writes it when it lives under `tests/` — then `just pre-commit`.
5. **Re-check once**, only the lines the fixes touched, with one `security`
   lane. A P0 or P1 still standing is a design problem: stop, and bring it to
   the owner with the evidence.
6. **P2** is fixed when cheap, else reported with its evidence; **P3** is dropped
   unless trivial.
7. **Report** each finding with its level, clause and disposition (fixed by
   which test, reported, dropped), then what was attacked and found clean
   versus what was never attacked.
