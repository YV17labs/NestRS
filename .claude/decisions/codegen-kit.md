# `nest-rs-codegen`'s root is a kit; the rest is `__private`

Through 7.0's foundation pass `nest-rs-codegen`'s root re-exported every
helper the framework's macro crates share — about 140 names, plus three public
modules — and four surfaces disagreed on what that was: its manifest said
internal and no dependency, its README invited third-party macro crates, the
packages page said never a direct dependency, and the upgrading guide taught
decorator authors `JobDecorator::grammar()`, `pair::HTTP` and
`transactional_value`. The crate is published, so every one of those names was
semver-public with no contract.

Now the root is the kit a third-party `proc-macro` crate builds a decorator
with, and semver covers it: `Grammar` and `Arg`, `Refusals`,
`duration_millis`, `pascal_case`, `snake_case`, `take_flag_attr`,
`take_single_attr`, `take_path_list`, `repeated_attribute` and `reroot` — what
a decorator needs that is no edge's own machinery. Everything else sits in the
crate's `__private`, which the framework's `*-macros` crates import.

- **A kit, not "internal"**: NestJS documents its decorator APIs, the guide
  already promised one, and an internal crate would have published its names
  anyway.
- **Left out on purpose**, each additive later: the pair table (the set of
  pairs is the framework's, a hard "no"); `PostureRules`, `Posture` and
  `take_meta` (a posture or a declared value means something only to a guard
  contract a third-party edge cannot have yet); the job table; the
  `#[injectable]` body builders (a third-party decorator puts `#[injectable]`
  on the struct instead); `versioning`; the handler and site declarations and
  `answer_span`, which emit or serve `nest_rs_core::__private`; the shared
  refusal sentences, which `Grammar` already words.
- **`Refusals` joined the kit** rather than `__private`: three macro crates
  wrote the same accumulator by hand, and a third-party decorator reading a
  whole item needs it as much.
- **`pair`, `layers` and `versioning` went private**, re-exported under
  `__private` as modules of the same name, since their callers spell
  `pair::HTTP`. A nested `pub(crate) mod __private` in each would have left
  three empty public modules.
- **The `clippy.toml` promise is kept**: `syn`'s three meta parsers are
  disallowed, and the one call site — sea-orm's own `#[sea_orm]` attribute —
  carries its `#[expect]`. Codegen calls none, so it carries none: an
  unfulfilled expectation fails `-D warnings`.
- **The kit names `syn` 3 and `proc-macro2` 1 types**, so a `syn` 4 is a
  nestrs major (`public-dependencies.md`); `syn` majors come years apart.
