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

## 2026-10-31 — a bound names only its range; counts get bounds; a retired key says it is retired

The entries above held a duration through bounds "whose key carries the unit
and is asserted at compile time to be the field's": every bounded field cost a
constant typing its own key and field path, which the derive already knew and
only compared. Under the owner's "Tout faire (Recommandé)" (2026-10-31, the
identity audit's second pass, its item 9, agreeing the public API break), a
range names its ends and their reasons alone — `DurationBounds::secs` or
`millis(floor, ceiling)`, `CountBounds::new(floor, ceiling)` — and
`#[config(bounds = RANGE)]` binds it to the field as the associated constant
named for it, a `BoundedSetting` over the field's type: its key the field's
name with the unit's suffix after it (the unit stays in the key; the developer
stops writing it), its path `Config::field`. The overlay reads through it, and
so does every site naming the variable or holding a value built in code, so no
site types the key; a hand-written overlay binds its own once. Off follows from
the field's type (`Option<Duration>` reads `0` as `None`), so
`Floor::UnitsOrOff` and `ConfigError::MisshapenDuration` are gone by
construction, aliases included — the refusal below of judging `Option` by its
tokens holds, since the trait impl on the real type answers. A count with a
reason at each end is a `CountBounds` on a `usize`, which retired three
hand-written readers (HTTP's connection cap, MCP's session cap, the outbox's
batch), and a `with = path` reader is handed its `Field`, key and path, for
the refusal of a value set in code — bound to the struct the way a range is, so
a check outside the reader types neither.

The previous entry's "refusing the boot on" a retired key no longer holds
whole: the nine security-header keys are refused at boot (B-160, security
first: a header the deployment set would go unsent where a `warn` is all it
would hear), and the refusal reads as a retirement,
`ConfigError::Retired` — "<VAR> is read by nothing since 7.0 — set <NEW>:
<why>" — never "invalid value", the key and not its value being refused. A
retired key the crate that retired it declares is named by the unread-variable
report in the same words at `warn`, as are OpenTelemetry's retired namespace
and the kernel's `<PREFIX>_LOG`; reading one stays refused (an alias).

Refused:

- **The key chosen at expansion** (the audit's "in const context by matching
  the range's unit"). A macro sees the range as a path, never its type, and no
  generic `const fn` joins two strings: the key is joined from the field's name
  and the range's unit when a read or a sentence needs it, at boot.
- **A stem passed at every read of a hand-written overlay**
  (`RANGE.read(env, "OPERATION_TIMEOUT", base)`): storage names its budget's
  variable in three runtime sentences, each of which would type the stem
  again. The overlay binds its setting once, as the derive does.
- **A range per field.** A range shared by several fields (an edge's unit
  timeout, a connection's lifetime ceiling, a stream's keep-alive) is one
  value, named on each field, since it names no key.

## 2026-11-05 — every count reads through its bounds

The 2026-10-31 entry bounded three counts and left the rest on
`#[validate(range(min = 1))]` and `#[config(count)]`: a count had two ways to be
bounded, a floor without a ceiling, and three (`ThrottlerConfig::limit`,
`SeaOrmConfig::{max_connections, min_connections}`) no bound at all. Under the
owner's "Tout faire (Recommandé)" (2026-10-31, the identity audit's second pass,
its item 9, agreeing the public API break), every count reads a `CountBounds`:
`BoundedField` gains `u32` and `Option<usize>`, whose `0` from the environment is
`None`, which absorbs `#[config(count)]` and `ConfigService::count`, both gone.

A `u32` member rather than the fields moved to `usize`: the field then holds what
its library takes (`PoolOptions::max_connections`, `Throttle::new`), so no site
casts, and no cast can fail or truncate past a ceiling. A `usize` would have been
a public type change for one `try_from` per use.

Each ceiling is decided by its values, not by a guess at the product: a ceiling
exists to catch a typo or an unbounded wait, never to refuse a real deployment.
`MAX_DEPTH` stops at 33 because async-graphql's recursion bound refuses a document
past it whatever the limit says; `MAX_CONNECTIONS` at 262143 because a Postgres
server's own `max_connections` does (`MAX_BACKENDS`); the body caps at a tebibyte
(a transport-wide cap a streamed upload reads under) and a gibibyte (what a socket
or an MCP POST buffers whole), memory being what they guard. No count here has
needed a floor alone: each has a real upper end.

Refused:

- **`u32` fields moved to `usize`**, above.
- **`SeaOrmConfig`'s pair judged in the overlay.** A combination is the consumer's
  (`container.md`), refused in `connect_options`, the one constructor every path
  reaches, where sqlx would clamp a minimum above the maximum in silence.

### Every key 7.0 renamed is declared retired

The 2026-10-31 entry declared the HTTP vocabulary's keys alone, so every other
rename stayed the report's guess (`OAUTH_RESOURCE` read as a misspelled namespace)
or its silence (`redis__worker` read as another binary's). A `RetiredKey` per key
of the upgrading guide's step 3 now says each, in the crate that retired it, under
`variable read by nothing since 7.0`, once the namespace that reads what it set was
read, so another binary's stays silent. A key may move namespace, or go with nothing
in its place; `read_as` then names the variable it became, or nothing.

The refusal is declared on the same line (`RetiredKey::refused`) and judged at
`read`, the seam every path into `from_env` takes, so a crate's refusal holds no
reader of its own: the nine security headers moved there from HTTP's. Refused
beside them, by "losing it leaves a policy weaker than the one set":
`<PREFIX>_GRAPHQL__DISABLE_INTROSPECTION`, whose successor is its opposite. Every
other key is said at `warn`, each read against the same question: the OAuth
client's and resource's and `STORAGE__BUCKET` stop the boot on their own, a required
value unset; the CORS keys and the bare `SECURITY_HEADERS` leave a policy more closed;
the emit's and the drain window's move no policy.

A refused key is judged by the tier that answers the read, and the judgement records
nothing, so no report calls it read and no near miss is told to set it. Under a pin the
cascade answers nothing and the pin holds the policy, so what a `.env` sets there
weakens nothing and is said retired; what the deployment sets is refused, as it is with
no pin. A refusal that fired under a pin on a file the read never reaches would refuse
an app whose policy is whole.

Refused:

- **A key's retirement named by value.** The refusal asks whether the key is set,
  never what it holds: reading the value to decide would be reading it.
- **`<PREFIX>_QUEUE__URL`, and the demo's `<PREFIX>_OAUTH__SIGN_IN_RESOURCES`.**
  6.0 retired the first, so "since 7.0" would be false of it; the second is a
  product's own key, which the framework's registry has no business naming.

