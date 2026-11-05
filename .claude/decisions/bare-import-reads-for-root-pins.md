# A module is imported bare to read its environment, and `for_root` only pins

Decided on 2026-10-31, from the identity audit's comp-6, cfg-9, rules-10,
new-11, api-13, comp-4, api-10, cfg-5 and rules-4, under the owner's "Tout
faire" of 2026-10-28, which agreed the public API break for 7.0.

"Read my variables" had two spellings on half the modules owning a
`#[config]` — `GraphqlModule` and `GraphqlModule::for_root(None)` queued the
same factory — and only `for_root(None)` on the other half (`SeaOrmModule`,
`RedisModule`, `ThrottlerModule`, `AuthnModule`, `OAuthResourceModule`, and
the family members the audit did not count, `OAuthClientModule` and
`OAuthServerModule`), so a developer had to know which was which, and `None`
was most of what a composition root said. A named instance was a value at its
opener (`RedisModule::for_root((Cache, None))`) and a type at every binding
(`RedisThrottlerModule::<Cache>`). And `ConfigModule::for_root()`, first in
every demo root, did one thing: register `Environment` and file the line
`undeclared-environment-is-production.md` promises — a line `nestrs new`'s
apps never filed, since its template left the import out.

**The bare import reads; `for_root` pins.** Every module owning a `#[config]`
implements `Module` by type, and its bare import queues the environment over
the defaults and does the module's whole wiring; `for_root` takes only a value
to pin — `C`, or `(Marker, C)` for an instance-capable module — and never
`None`. A named instance is a type argument wherever it is imported bare
(`RedisModule::<Cache>`), as every binding already was; its value stays
`(Marker, C)` at `for_root`, since Rust infers no default type parameter in an
expression. `OAuthServerModule` is generic over its payload, so its bare form
is `OAuthServerModule::<ClientPayload>`. `OAuthResourceModule` loses its
private host: it hid the `#[module]` only because a bare import would have
done nothing, and the bare import now does everything.

**The primitive splits by what it queues.** `ConfigModule::provide_feature::<I,
C>(builder)` queues what a bare import reads, `ConfigModule::provide_root(x,
builder)` the declaration a `for_root` pins — the for-feature/for-root split
`config-one-seam.md` keeps, read off the same words. `ForInstance<I, C>` holds
a pinned config only, so `RedisModule::for_root(None)` does not compile.

**The environment needs no import.** Both primitives register the active
`Environment` the first time a boot loads a config, filing the undeclared
line then, so every app that reads a config — every app `nestrs new` writes —
says it once. `ConfigModule::for_root()` is gone; the rule that called it "the
`.env` cascade, first in the root's imports" had drifted from code that read
the cascade without it.

**A declaration with no environment twin folds into the setup.** MCP's
identity and its opening, and static files' embedded folder, are code with no
variable. Their `*Options` wrappers, a third configuration word beside
`*Config` and `*Setup`, are gone: `for_root` takes `impl Into<MSetup>`, the
setup opaque, and the conversions are its only constructors — the declaration
alone, the config alone, or the declarations before the config as a tuple, as
a marker precedes its config (`McpModule::for_root((McpAccess::Public,
identity))`, `StaticFilesModule::for_root((Embedded::of::<Web>(), cfg))`).
MCP's opening is `McpAccess::{Gated, Public}`, so a posture the boot used to
refuse (`InBand`, `Ambient`) no longer compiles, and `Public` stays the one
visible line that opens it.

**Refused:**

- **Keeping `for_root(None)` beside the bare import.** Two spellings of one
  import, and `None` a word typed to say nothing; adding the `Module` impls
  alone would have been additive and left it standing.
- **A turbofish at `for_root`** (`RedisModule::<Cache>::for_root(cfg)`):
  `named-instances.md` refused it, and nothing here needs it.
- **One primitive taking `Option<ForInstance<I, C>>`**, or `ForInstance`
  keeping a "nothing pinned" state: either lets `for_root` accept the
  environment-only form again, through a type a reader must look up.
- **`McpOptions` renamed** (`McpServer`, `McpDeclaration`): still a third word
  for one value, with a load-bearing `Option` inside it.
- **A builder method on the setup** (`.access(McpAccess::Public)`) and **a
  typed opening** (`McpModule::<Public>`): the first is the chain
  `container.md` refuses; the second would make the opening a type a feature
  crate could name as easily as the app, and dedup two openings silently
  where a declaration contests them.
- **Every importer of a config importing a private `ConfigRoot`**: a module
  imported in `collect` alone is refused at the register phase, and the
  primitives already run once per config a boot loads.

Held by `nest-rs-testing`'s setups table (every module bare, pinned, both in
either order), `nest-rs-config`'s environment tests (two readers of two
configs, no import, one line), and the types: no `for_root` takes an `Option`.

## 2026-10-31 — `OAuthServerModule::<P>` provides the client authentication it reads

`OAuthServerModule::<P>` stays generic, and its bare form stays
`OAuthServerModule::<ClientPayload>`; what the import provides grows. It
registered only its config, so every app issuing tokens assembled the one
§2.3.1 strategy the crate ships and its guard from three generic parts —
`ClientSecretBasicStrategy<P>`, `ClientAuthenticationGuard<S>`, and an alias
of its own naming both — and listed them. Decided on the owner's agreement to
the identity audit's second pass ("Tout faire (Recommandé)", 2026-10-31, its
item 11), which covers this public API break: the module registers
`ClientSecretBasicStrategy<P>` and its guard, named once by the framework as
`nest_rs::oauth::server::ClientGuard<P>`, in its own `register` — a `#[module]`
is not generic — so a token endpoint binds `#[use_guards(ClientGuard<P>)]` and
its app lists neither. An app still listing either registers it twice, which
fails the boot naming both registrations (`DuplicateProviderError`).

`SocialModule` does not do the same for `SocialStrategy<S>`: it is not
generic, and taking the product's `SocialSignIn` type would make every import
of it — each provider's module imports it — a turbofish. JWT's guard alias
stays the product's for the same reason: `AuthnModule` also provides the
`JwtService` that modules import it for.

## 2026-10-31 — a resource server's scopes are declared in code, its variable only replaces what it advertises

The scopes an app's rules gate on were plain strings typed four times: the
rules' constants, `<PREFIX>_OAUTH__RESOURCE__SCOPES_SUPPORTED`, each
registered client, the tests. A drift between the first two surfaced only as
a `warn` on a refused request. Decided on the owner's agreement to the
identity audit's second pass ("Tout faire (Recommandé)", 2026-10-31, its item
11), which covers this public API break:

- **A scope is `nest_rs::guards::Scope`**, a newtype over RFC 6749 §3.3's
  token, built by `Scope::new` and refused outside the grammar — at compile
  time in a `const`, as `DurationBounds` refuses a key. It sits in
  `nest-rs-guards`, below `nest-rs-authz` and `nest-rs-oauth-resource`,
  beside `GrantedScopes` and `RequiredScopes`; the umbrella's `authz` feature
  brings `guards`. `RuleSpec::requires_scope` takes a `Scope`, so a rule
  cannot require a scope no declaration holds.
- **`OAuthResourceModule::for_root` takes the declared set** — `[Scope; N]`,
  the config, or `([Scope; N], config)` — and the document and the challenge
  advertise it when the deployment names no `SCOPES_SUPPORTED`. It is a
  declaration with no environment twin: the variable says what one
  deployment advertises, and replaces the list, never the vocabulary the
  rules are written in. Two declared sets contest at boot.
- **A registered client's scopes stay strings**: they are deployment data,
  and the product's check of them against its set stays in its module-init
  hook (`token-endpoint-client-refusal.md`).

Refused: **a type parameter on `OAuthResourceModule`** (a set as a trait's
associated constant): a `#[module]` is not generic, the access graph reads
only static descriptors, so the module's controller would leave the graph a
transport gates discovery on. **Pinning the list in
`OAuthResourceConfig`**: a pin moves the whole namespace to
deployment-over-pin, so the cascade's resource and issuers would no longer
be read. **Scopes discovered from the rules**: a rule is evaluated per
actor, and nothing evaluates them at boot. **`requires_scope` keeping a
string beside the type**: two spellings, one of which can drift.

## 2026-11-05 — `McpAccess` holds `Public` alone

The paragraph above names `McpAccess::{Gated, Public}`. The type holds `Public`: gated is what every import leaves, so no line spells it and none reads as a closing another import's opening would override. `McpAccess::Gated` does not compile.
