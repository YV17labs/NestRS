# A config's overlay is read off its struct

`#[config]` writes `Config::from_env`: each field reads the variable its name
spells in SCREAMING_SNAKE, under the namespace the struct declares, through the
reader its type chooses (`ConfigField`, `FromSetting`); a `Duration` reads the
`DurationBounds` its field names, whose key carries the unit and is asserted at
compile time to be the field's; `json`, `count` and `with = path` say what a
type cannot. `from_env = "manual"` keeps a hand-written overlay for fields that
depend on each other (storage's backend-gated settings) or a generic config.
The struct is the whole contract, and no field is reachable from code alone.

Refused:

- **The hand-written overlay as the rule** (the configuration page's former
  "Why hand-written `from_env`": an auto-derived contract is implicit). The
  derive reads every key off the struct, which is more explicit than a line
  per field typed apart from it: a forgotten line made a field code-only, a
  retyped key drifted from its field, and only review held either.
- **The namespace read off the file's path by the macro.** A family's `__`
  against a theme's `_`, a plural role folder (`providers/`) and a product's
  container are judgments a macro cannot make
  (`port-config-namespace.md`, kept): the namespace stays written, and the
  missing-namespace error words the stem rule with its examples rather than
  prescribing one crate's.
- **A nested vocabulary's keys joined with `__`.** `__` is the namespace's
  separator, each segment a folder of the stem; a key holding one reads as a
  sibling namespace: the unclaimed-variable report takes it for another
  binary's sub-namespace and stops naming its misspellings, and a field
  `queue` of `RedisConfig` would read `<PREFIX>_REDIS__QUEUE__*`, the queue
  binding's own namespace. A vocabulary of several variables extends its
  field's key with `_` (`TLS_CA_CERT` under `tls`), as `TLS_`, `CORS_`,
  `CONTACT_` and `LICENSE_` already did, so no deployment variable moved.
- **A serde deserializer over the environment** (config-rs, figment): it
  reads neither `<KEY>_FILE`, nor the tiers a pin narrows, nor the claim
  registry, and its errors quote the value.

**A duration has one shape**: a `Duration`, or an `Option<Duration>` where
`None` turns it off and means nothing else; the default sits in `Default`, and
no field names a unit — the key does, through its bounds. **A count follows**:
`u32`/`usize` with its default in `Default`, or an `Option` whose `None` is
unlimited, read `0` from the environment (`count`), a pinned `Some(0)`
refused. `None` meaning "the library's default" (SeaORM's pool, the
throttler's window, the JWT leeway) was one word for two concepts across one
set, so a pin had to learn each struct's meaning.

Decided 2026-10-30 by the identity audit's feat-12, cfg-6, cfg-12 and read-6,
the owner's "Tout faire" (2026-10-28) agreeing the public API break.
