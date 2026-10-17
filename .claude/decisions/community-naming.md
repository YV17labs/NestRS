# A community package is `nestrs-<subject>`, and every surface it owns is rooted at its crate

`nest-rs-*` is the framework's, recognised by one closed table (`nest-rs-codegen`'s
`FRAMEWORK`, one row per crate the umbrella re-exports) and by nothing else. A
package someone else publishes takes the other spelling of the brand, and
derives every name an operator types from its crate name:

| Surface | Framework | Community package |
|---|---|---|
| crate | `nest-rs-<concern>` | `nestrs-<subject>` |
| path | `nest_rs::<concern>`, through the umbrella | `nestrs_<subject>::…`, its own root; the umbrella is closed |
| span target | `nest_rs::<concern>` | `nestrs_<subject>` or `nestrs_<subject>::<concern>` |
| unit | `<edge>.<unit>` | `nestrs_<subject>.<unit>` |
| config namespace | `<PREFIX>_<CONCERN>__*` | `<PREFIX>_<SUBJECT>__*`; a collision fails the boot |
| datastore key | `nestrs:<concern>:<structure>…` | `nestrs_<subject>:<structure>…` |
| crates.io keyword | `nestrs` | `nestrs` |

Read off NestJS's split: official packages are `@nestjs/*`, community ones
`nestjs-*` (`nestjs-pino`, `nestjs-cls`). The two spellings differ at their
fifth byte (`nest_rs` against `nestrs`), so `nest_rs=off` never silences a
community target and `SCAN nestrs:*` never matches a community key — the
raw-prefix law of `operation-target.md`, held across the boundary.

Before this, three mechanisms decided "framework or not" by the `nest_rs_`
prefix — the macro root rewrite, the inert-host level and the unit check — and
the driver pages told a third party to take that prefix. A crate following the
docs was rewritten to a path the umbrella does not have and reported at the
framework's level. The rewrite now reads the table, the inert-host level reads
the app's composition, and the unit check reads the declaring crate's name.

## Refused

- **Community crates share `nest-rs-*`** (Bevy's model). `nest_rs=off` would
  silence every community target, and the closed table would have to tell its
  rows from lookalikes forever.
- **Free names, with only the `nestrs` keyword.** Targets, namespaces and keys
  would be chosen per package, not derived: a reader could not place the next
  one.
- **A security argument for the prefix.** crates.io has no namespaces, so no
  prefix stops a lookalike crate; the publisher's identity does, and the
  package page says to check it.

## Known limits

- Two community crates whose names prefix each other (`nestrs_kafka`,
  `nestrs_kafka_streams`) share `EnvFilter` matches, as `tokio` and
  `tokio_util` do. The unit check refuses the shorter's target under the
  longer's root, which is all a framework can hold.
- A community namespace can collide with a first-party one added later in 7.x
  (a `nestrs-mail` taking `MAIL` before a first-party mail crate): the boot
  refuses the pair (`SharedNamespace`). A new first-party namespace is a new
  crate, which the owner decides, and that decision checks crates.io for
  `nestrs-<word>` first.
