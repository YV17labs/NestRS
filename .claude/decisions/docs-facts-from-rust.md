# The docs lint reads facts derived in Rust, regenerated at lint time

The docs linter once re-derived framework facts in JavaScript with regexes over
`crates/**` and `demo/**`. Two implementations of one definition drifted: the
linter counted 27 capabilities against a landing page that correctly said 28,
while its own comment claimed the two could not disagree. The facts moved to
one Rust derivation.

That derivation was first a test that rewrote a committed `docs/canon.json`
and failed until the file was committed. Half of that file's commits only
bumped a test count, parallel branches collided on it, and a framework change
failed a docs check for a reason no reader of the diff could see. It became a
generator the lint runs, so the facts are as old as the tree and nothing is
committed to satisfy a test.

The lint's baseline once had an `--update-baseline` flag that re-snapshotted
every violation, code-truth ones included, so the remedy its failure message
printed turned a proven-false public claim into a permanent exemption. It was
removed: a baseline line is added by hand, where a reviewer sees it.
