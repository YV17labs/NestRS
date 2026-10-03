# The rules are a budgeted map, not a manual (7.0)

During 7.0 `CLAUDE.md` reached about 500 lines and `architecture.md`, which
every scaffold ships as its `AGENTS.md`, about 450. Anthropic's guidance for
Claude Code memory is a `CLAUDE.md` under 200 lines holding what a session
needs every time — commands it cannot guess, decisions specific to the project,
gotchas — with path-scoped rules for the rest, hooks for what must always
happen, and emphasis kept for the few things that warrant it. OpenAI's agent
guidance is a map of about a hundred lines. The commonest rules-file smell in
the research was "lint leakage": prose for what a linter or a type already
enforces, which drifts from the holder and dilutes the rules that need reading.

So (owner decision, 2026-10-03): `CLAUDE.md` 200 lines, `architecture.md` 250,
each zone rule 300, emphasis dialled back, and a rule a type or lint holds
written as a pointer to it. The cut moved decisions rather than dropping them:
what binds part of the tree went to the zone file for those paths (`demo.md`
for the product, `manifests-ci.md` for shipping a capability, `testing.md` for
what `nest-rs-conformance` holds, `observability.md` for the target table), how
rules are written went to `rules.md`, and the history of each refused
alternative stays here.

`nest-rs-conformance` was removed the same day
(`.claude/decisions/conformance-scanner.md`); what `testing.md` said it held
went with it.
