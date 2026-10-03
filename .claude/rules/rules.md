---
paths:
  - "CLAUDE.md"
  - ".claude/**"
---

# Writing the rules

`CLAUDE.md` is the map every session loads: the project's context, what no
session may get wrong, and where to look. Everything else is a rule file —
loaded with its `paths:`, or always when it has none (`naming.md`,
`architecture.md`). `.claude/decisions/` is the history, never loaded.

- **A rule states a principle that holds in every case**, with its reason when
  the reason is not obvious — the reader is a model that can apply it. An
  enumeration stays only when a tool parses it (the reserved vocabulary) or the
  set is closed by design (the edges); counts and thresholds are not rules.
  Rustdoc states mechanics, `.claude/decisions/` the alternatives refused — a
  rule never carries its own history, never restates code, and never names an
  item path a reader cannot open.
- **A rule names its rung** (`CLAUDE.md`, *How a rule is held*). What a type, a
  lint or a test holds is one pointer to the holder, never a paragraph
  explaining it again.
- **A rule is added or changed only** when two rules disagree or the same class
  recurs in an unrelated change. A finding is fixed with its regression test,
  not with a sentence.
- **A rule goes where it binds**: part of the tree is a zone file for those
  paths, and `CLAUDE.md` keeps what any session would get wrong without it —
  commands it cannot guess, decisions specific to this project, gotchas. Moving a rule keeps it; deleting one is a decision of its
  own.
- **Emphasis is for the security invariants.** Bold and absolutes lose their
  force when every line carries them.
- **Every loaded line costs every session that loads it.** A rule that adds
  nothing a capable reader would not do anyway is cut; a growing file merges
  before it adds.
- **`architecture.md` has readers that parse it** (`cli.md`, *Templates*):
  change a table row, a heading or the reserved-vocabulary fence together with
  them, never alone.
