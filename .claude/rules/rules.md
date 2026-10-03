---
paths:
  - "CLAUDE.md"
  - ".claude/**"
---

# Writing the rules

`CLAUDE.md` is the map every session loads; a zone rule loads with its `paths:`;
`architecture.md` loads always and ships as every scaffold's `AGENTS.md`;
`.claude/decisions/` is the history, never loaded; a procedure is a skill.

- **A rule states a decision**, with its reason when the reason is not
  obvious. Rustdoc beside the code states mechanics and numbers, and
  `.claude/decisions/` the alternatives refused — a rule never carries its own
  history, never restates code, and never names an item path a reader cannot
  open.
- **A rule names its rung** (`CLAUDE.md`, *How a rule is held*). What a type, a
  lint or a test holds is one pointer to the holder, never a paragraph
  explaining it again.
- **A rule is added or changed only** when two rules disagree or the same class
  recurs in an unrelated change. A finding is fixed with its regression test,
  not with a sentence.
- **A rule goes where it binds**: part of the tree is a zone file for those
  paths, a procedure is a skill, and `CLAUDE.md` keeps what any session would
  get wrong without it — commands it cannot guess, decisions specific to this
  project, gotchas. Moving a rule keeps it; deleting one is a decision of its
  own.
- **Emphasis is for the security invariants.** Bold and absolutes lose their
  force when every line carries them.
- **Budgets**: `CLAUDE.md` 200 lines, `architecture.md` 250, each zone rule
  300. A file at its budget merges or drops a rule before it gains one.
- **`architecture.md` has readers that parse it** (`cli.md`, *Templates*):
  change a table row, a heading or the reserved-vocabulary fence together with
  them, never alone.
