# The conformance scanner was replaced by types, lints and behaviour tests (7.0)

Through 7.0's development `nest-rs-conformance` grew from 7.2k to 17.9k lines
of `syn`-based "joins": tests that read the framework's own source to prove a
rule was followed or a family member was named by a test, plus a `blinds` join
proving the other joins could not be evaded.

Three adversarial audit rounds kept finding constructions that hid a member
from a join — raw identifiers, `include!`, `cfg_attr`, glob re-exports,
qualified-self paths, `macro_rules!` — and each fix added scanning. A survey of
all 27 joins found that none had caught a production defect after it landed:
every defect a join cited had been found first by a human, an audit or a
behaviour run. 47 of the release's 281 commits touched the crate, most of them
to fix a join's own reading, and rules prose was bent to satisfy the scanners.

A source scanner sees spellings; the compiler resolves items. So the rule is
now held at the strongest rung that can hold it (CLAUDE.md, *How a rule is
held*): a type that makes the wrong code unwritable, a rustc/clippy lint (whose
`disallowed-*` lists match resolved paths and were proved against every
evasion above), a behaviour test whose assertions `cargo mutants` checks, or
review. Run on one crate, `cargo mutants` found nine untested boundary
conditions that 39 green tests and every join had missed.

What remains in `nest-rs-conformance` reads file paths and manifests only.
