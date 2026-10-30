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

## 2026-10-30 — every key read off a field's name, `ClientTls` under `tls` alone

The entry above kept two vocabularies whose keys were not their members'
names: `HttpCors::exposed_headers` read `CORS_EXPOSED`, and
`HttpSecurityHeaders` read its switch under the bare `SECURITY_HEADERS` and
each header under a flat key (`HSTS`) beside it. Under the owner's "Tout
faire", they now read `CORS_EXPOSED_HEADERS`, `SECURITY_HEADERS_ENABLED` and
`SECURITY_HEADERS_<FIELD>`: a vocabulary's keys are its members' names after
its field's key, so every variable is derived from the structs but the
client-TLS one's (below), and "no deployment variable moved" above no longer
holds for these two. The retired
spellings are read by nothing: the unread-variable report names each at boot,
as it names every retired key of a namespace the binary reads, with the key it
became as its suggestion — the one read key it leads (`CORS_EXPOSED`) or ends
at a segment (`HSTS`), each too far by edit distance for a third of its length
to reach. A key that leads several read keys (the bare `SECURITY_HEADERS`)
gets none, where the nearest by distance would have told an operator to write
`SECURITY_HEADERS_HSTS=false`.

A `ClientTls` is read under a field named `tls` alone, another refused at the
read (`ConfigError::MisnamedField`): every check of its material, the
handshake's remedy and the drivers' own refusals name `TLS_CA_CERT`,
`TLS_CERT` and `TLS_KEY`. It is the one vocabulary whose keys are not its
members' names: they name the material every TLS client presents, the client
vocabulary the container rule fixes, where its members are `authorities` and
an `identity` holding `cert` and `key`; every other vocabulary is read off its
members, `HttpTls`'s `reload` as `TLS_RELOAD_SECS` among them.
`HttpTls::with_reload_secs(u64)`, whose `0` stopped the watch, is
`with_reload(Option<Duration>)`, the duration shape above.

The shape is held where the field is read: the derived read refuses an
`Option<Duration>` whose bounds admit no off — its `None` would mean "unset"
again — and a `Duration` whose bounds turn it off at `0`
(`ConfigError::MisshapenDuration`), and a `#[config(count)]` pinned at
`Some(0)` with its variable unset, as a pinned zero duration is refused.

Refused:

- **Carrying the key in `ClientTls`**, so its sentences name whatever field
  read it. Every sentence naming its variables — `TlsNames`, the remedy a
  failed handshake gets, Redis's plaintext refusal — would take the key as a
  second input, and a namespace would spell its TLS material as its author
  chose. One spelling in every namespace keeps each sentence true with one
  naming path and lets an operator audit every client's material under the
  same three keys (security, correctness); a config holding two TLS clients
  holds two resources, each its own namespace.
- **The shape checked at compile time.** The derive sees a field's type as
  tokens, so it would judge `Option` by its spelling, blind to an alias; an
  assertion on the type itself is a second obligation on it, and a field of
  a type no bounds read would be reported twice. The read refuses every
  shape, aliases included, at the first boot or test that reads the config.
- **Reading a retired key, or refusing the boot on one.** Reading it is an
  alias (`CLAUDE.md`, no alias inside a major); refusing it is a detector that
  keeps the 6.x name in code for a deployment the report already names at
  `warn`, the level every unread key earns (`container.md`, escalate no
  further than the fact). A policy header set under its old name stops being
  sent, which the upgrading guide's *What fails* says.

Decided by correctness and security, the orchestrator's resumption of the
identity audit's branch (B-152).
