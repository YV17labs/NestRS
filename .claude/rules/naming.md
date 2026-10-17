# Naming

A name is a coordinate, not a label: from a type you find its file, from a path
its type, and a reader who knows the convention guesses the next name without
asking. You cannot check a name alone — a name judged alone always passes — only
the set it joins. `architecture.md` applies this to an app's modules, files and
types; these principles cover every name. Rust's own conventions (rustc lints,
API guidelines) come first.

## Every name

Each principle has a test; a name that fails one is not written.

- **Judge it in its set.** Name two siblings that will plausibly follow. Cannot
  find two? The axis is not found yet.
- **Read the shared part off something real** — a standard's vocabulary, the
  domain's word, the path, the owning type — never invent it. Test: someone who
  did not write the code can say whether a candidate belongs.
- **Most stable segment first.** Listings sort on the raw string, so segment
  order is the grouping (`TOKEN_EXPIRED`, `TOKEN_REVOKED`, never
  `EXPIRED_TOKEN`). No name is a raw prefix of an unrelated one: log filters
  and `SCAN` match with `starts_with`.
- **The next member has exactly one place.** None means the scheme is closed;
  two mean two axes crossed — fix their order once (vendor, then port).
- **One word per concept, one concept per word**, across the whole set. A rename
  that leaves a sibling behind (`*Setup`, `*Config`, the variable, the docs) is
  half a rename.
- **Say what it refuses.** `utils`, `common`, `helpers`, `manager`, a catch-all
  `service` refuse nothing and fill forever.
- **Length follows reach**: a closure argument can be `x`; a field, a public
  item or anything an operator reads says what it holds.
- **Round-trip**: given the rule and no member, a fresh reader derives the name.
  A set that cannot be derived is a lookup table, and drifts.

## A name that leaves its crate or reaches an operator

Public types and error variants, modules and crates, env vars and config keys,
span targets, storage keys, CLI commands and flags. These are permanent once
someone greps them, so before writing one, state its set — in the reply to the
owner or the commit body:

- the set in one sentence, and where its shared segment is read off;
- its members, existing and coming, sorted as a reader meets them;
- the one-line rule that places the next member, and what it refuses;
- what holds it: a typed constant or enum, `nestrs lint`, or review.

| Surface | The set is | Read sorted in |
|---|---|---|
| public type | the siblings filling one seam | the module index, a stack trace |
| module, crate | what one directory shows | `ls`, `cargo tree` |
| env var, config key | everything one deployment sets | `env \| sort`, a chart's values |
| span target | every filter an operator writes | a log filter, a query |
| storage key | everything one backend holds for the app | a `SCAN`, a schema dump |
| CLI command, flag | everything `--help` prints | `--help`, completion |
| error variant | every outcome a caller matches | the `match`, the error docs |

Two attempts yielding two schemes mean a fact about the domain is missing, not
a better word: find the fact. A new crate, crate family or edge is proposed
with its set and decided by the owner.
