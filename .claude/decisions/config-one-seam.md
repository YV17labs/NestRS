# A config has one seam, and the loader never arbitrates

`for_root` configures a module; `for_feature` registers a config and takes no
value. This is NestJS's split (`forRoot` configures once, `forFeature` registers
artifacts against an already-configured module). During 7.0 `for_feature`
briefly took a pinned base too, which made every module-owned config reachable
two ways, so its value depended on `imports = [..]` order; it was removed.

Two other loader-side shapes were tried and removed for the same reason — they
put a judgement in the crate that only loads:

- **Ranking the tiers of related variables inside the loader** (half a key pair
  in the deployment, the other half in `.env`). The consumer that uses them now
  refuses an inconsistent combination in its one constructor, naming what is
  set.
- **A second duration reader, `ConfigService::seconds`**, which read connection
  ceilings with no ceiling of their own, beside three hand-written range checks;
  one of them skipped the pinned value and booted a zero Redis budget. Every
  duration is now a `DurationBounds`, refused in the same words from the
  environment and from code.

Revisited (2026-10-30): `for_feature` left the public surface. A config a
provider of a reached module requires (`#[inject] Arc<C>`) is loaded by the
boot itself: `#[config]` files the type, and once every module has collected,
the container queues the environment-over-defaults factory of each filed config
such a provider injects, unless an import already queued one — a `for_root`
pin still supersedes it as a declaration. A framework module reading its config
in `register` without injecting it queues it from its own imports through the
config crate's private seam; a third-party module calls `provide_feature`. The
import line repeated what `#[config]` on the type and the provider's injection
already declare (the identity audit's comp-10, the owner's "Tout faire" of
2026-10-28 agreeing the break), and keeping the public `for_feature` beside
the inference would have been two ways to register one config. Nothing pins
through the inference, so the order-dependence this entry retired does not
return, and module-gating holds: the load is keyed on a reached module's
provider, and loading a config mounts and exposes nothing. Decided by elegance
— nothing to wire — under the security and correctness lines, which it leaves
where they were. The split stands: `for_root` configures, and a config the app
owns needs no registering seam at all.

Revisited (2026-10-31): the bare import reads; `for_root` only pins. Half the
modules owning a config had a bare form and half did not, so
`X::for_root(None)` and `X` were two spellings of "read my variables" on one
and the only one on the other. Every such module now implements `Module` by
type, its bare import the environment-only form, and `for_root` takes a value
to pin and never `None`; the primitive a driver calls splits the same way,
`ConfigModule::provide_feature::<I, C>` for what a bare import queues and
`ConfigModule::provide_root` for what a `for_root` pins
(`.claude/decisions/bare-import-reads-for-root-pins.md`). The split this entry
holds stands, read off the same two words: one value configures, and nothing
else does.

## 2026-11-05 — a setup has no public conversion

The owner's mandate of 2026-11-05. `McpSetup`, `StaticFilesSetup` and
`OAuthResourceSetup` took their values through `impl Into<…Setup>`, so `From` on the
setup was tier 1: `McpSetup::from(config)` imported the module beside
`McpModule::for_root(config)`, the second way to configure a module the hard "no"
refuses, and one a type can refuse. Each `for_root` now takes
`impl nest_rs_core::__private::ForRootValue<…Setup>`, implemented for the same values,
so a call site writes what it wrote. The third occurrence is what made it one trait.

**The price:** the bound names a tier-2 trait in a tier-1 signature, so rustdoc shows
it unlinked, and each `for_root`'s documentation lists its values, as it already did.
A package with a declaration beyond its config writes a function per value or a trait
of its own: nothing it names may sit under `__private`, and no package has asked.

**Refused:** `ForInstance<I, C>`'s conversion stays public. It is no setup: it reaches
nothing but a `for_root`, and a marker before a config is what it is for.

