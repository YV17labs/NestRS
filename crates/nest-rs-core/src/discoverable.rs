//! The [`Discoverable`] trait — the contract a `#[module]` uses to register a
//! provider and report its dependencies to the access-graph check.

use std::any::TypeId;

use crate::container::{ContainerBuilder, KeyedDependency};

/// Anything a `#[module]` can pull in via `providers = [...]`.
///
/// Decorator macros (`#[injectable]`, `#[interceptor]`, `#[scheduled]`,
/// `#[mcp]`, `#[routes]`, …) emit a single `impl Discoverable for Self` that
/// either registers a provider or attaches discovery metadata.
pub trait Discoverable {
    /// Provider types that must already be registered before
    /// [`register`](Discoverable::register) can build this one — read by
    /// `#[module]` to order registration. Empty for providers built lazily
    /// (controllers, resolvers) so they do not block the register-phase
    /// fixpoint.
    fn dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject]` dependency, recorded for the access-graph
    /// check. Reported regardless of build timing, so the contract governs
    /// transport-built logic too.
    fn injected() -> Vec<TypeId> {
        Vec::new()
    }

    /// Human-readable label for each [`injected`](Discoverable::injected)
    /// entry, in the same order, so the access graph can name a dependency no
    /// module provides — a lazily-built provider's missing dependency is a clean
    /// boot error naming both the provider and the dependency, not a
    /// `get(...).expect(...)` panic at first resolution. May be shorter than
    /// `injected()` (a provider that does not emit names falls back to a
    /// placeholder); never longer.
    fn injected_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// [`ProviderKey`](crate::ProviderKey) of each **keyed** `#[inject(key = "…")]` dependency,
    /// recorded for the access-graph keyed check. Kept apart from
    /// [`injected`](Discoverable::injected) — a keyed dependency is validated
    /// against the global keyed set (seeds + factory outputs), and its boot
    /// error names both the type and the key. Empty for providers with no keyed
    /// dependency (the default).
    fn injected_keyed() -> Vec<KeyedDependency> {
        Vec::new()
    }

    /// Human-readable label for each [`dependencies`](Discoverable::dependencies)
    /// entry, in the same order, so the boot-time fixpoint can name a missing
    /// dependency.
    fn dependency_names() -> Vec<&'static str> {
        Vec::new()
    }

    /// `TypeId` of each `#[inject] Option<Arc<…>>` optional dependency.
    /// Not required by the register-phase fixpoint, but
    /// used to order the provider after an optional dependency the same module
    /// supplies.
    fn optional_dependencies() -> Vec<TypeId> {
        Vec::new()
    }

    /// Container keys this provider registers **besides itself**, each with the
    /// label a boot error should use for it.
    ///
    /// A provider normally registers exactly one key — its own type — and
    /// `#[module]` records that automatically. This hook is for the provider that
    /// also installs a *typed singleton on its module's behalf*, so the access
    /// graph can attribute that key to the module and produce the same named
    /// error it gives for any other unimported dependency. `nest-rs-ws` uses it
    /// for the per-namespace `WsServer<N>` registries `WsModule` owns: without
    /// it, the key belongs to no module, and the graph's escape hatch for
    /// imperatively-registered types waves the dependency through — then the
    /// consumer panics at first resolution, naming the wrong provider.
    ///
    /// Empty for every ordinary provider.
    fn also_provides() -> Vec<(TypeId, &'static str)> {
        Vec::new()
    }

    /// Install this provider's construction into the builder — the register
    /// phase's per-provider step. Emitted by the decorator (`#[injectable]`,
    /// `#[routes]`, …); resolves the provider's dependencies from the builder
    /// and stores the built value plus any metadata.
    fn register(builder: ContainerBuilder) -> ContainerBuilder;
}

/// What the container holds under a provider's **own type**, stated by whatever
/// decorator built it — and read by the impl halves that resolve their host
/// there (`#[hooks]`, `#[scheduled]`, `#[listeners]`, `#[indicators]`,
/// `#[processor]`).
///
/// Those five resolve with `Container::get::<Host>()` at boot, at a tick, at a
/// published event, at a probe, at a job — always outside any request. That
/// call answers correctly for exactly one registration shape, **a singleton
/// stored under its own type**, and each of the others fails in its own quiet
/// way:
///
/// | Host | What `get` does | Symptom if it were allowed |
/// |---|---|---|
/// | edge host (`#[controller]`, `#[gateway]`, `#[resolver]`, `#[mcp]`) | `None` — the type registers *metadata*, an instance is built at mount | the edge serves, the method never runs, one `warn` |
/// | `#[injectable(scope = request)]` | `None` — the container holds a factory, not a value | the same `warn`, misnaming the cause |
/// | `#[injectable(scope = transient)]` | builds a **throwaway** instance | the method runs, its effects are dropped, and nothing warns at all |
///
/// The fact is **stated, never omitted**: every decorator that builds a provider
/// writes this impl, `true` or `false`. That is what makes contradicting it a
/// coherence error rather than a second opinion — a marker merely *absent* for
/// the shapes it refuses can be filled in by hand, and was. The escape hatch
/// survives only where nothing has spoken, on a provider registered by hand with
/// [`ContainerBuilder::provide`]:
///
/// ```ignore
/// impl ProviderResidency for MyHandWrittenProvider {
///     const SINGLETON: bool = true;
/// }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a provider, so a provider-hosted decorator (`#[hooks]`, \
               `#[scheduled]`, `#[listeners]`, `#[indicators]`, `#[processor]`) cannot reach it",
    label = "the container holds no singleton of this type",
    note = "those decorators resolve their host with `Container::get::<Self>()`, outside any \
            request, so the host must be a provider the container holds under its own type. Add \
            `#[injectable]` — or, on a provider registered by hand with `ContainerBuilder::provide`, \
            write `impl ProviderResidency` for it with `const SINGLETON: bool = true`"
)]
pub trait ProviderResidency {
    /// `true` when the container holds **one instance of this provider, under
    /// this very type, for the application's lifetime** — `false` for every
    /// other shape in the table above.
    const SINGLETON: bool;
}

/// The one sentence every discovery site appends when it skips an operation
/// whose host the booted container does not hold — lifecycle hooks, scheduled
/// methods, event listeners, health indicators, queue processors.
///
/// Shared because it is one refusal. Its wording is the third attempt, and the
/// first two are why it now **names causes and prescribes nothing**:
///
/// * *"provider unreachable from app's module tree"* was false whenever the
///   provider was written right there in `providers` — bound as `dyn Trait`,
///   imported through a `for_root`, or registered by a hand-written `Module`.
/// * Naming the dyn case and offering *"list it under its own type as well"*
///   was worse: `providers = [Foo, Foo as dyn Trait]` runs the provider's
///   constructor **twice**. The decorators then fire on one instance while every
///   consumer injecting `Arc<dyn Trait>` holds the other, and nothing warns —
///   the exact silent shape [`ProviderResidency`] exists to refuse, reached by
///   *following* the advice. On a hand-written `impl Module` the same edit
///   fails the boot outright with `DuplicateProviderError`.
///
/// A remedy is only safe to print where the framework knows which of the five
/// causes it is looking at, and at this site it does not: the container simply
/// has nothing under that type. So the sentence hands the developer the causes
/// and lets them pick. Anything narrower has been wrong twice.
///
/// Whether `providers = [Foo as dyn Trait]` should register one instance under
/// both keys — which would make the dyn case work rather than warn — is an open
/// design question for the owner, not something this constant may pre-empt.
pub const INERT_HOST_HINT: &str = concat!(
    "nothing is registered under that exact type. Common causes: its module is not imported ",
    "by this app (import it, or delete the methods); it is bound only as `dyn Trait`; it is ",
    "imported through a `for_root`; it is registered by hand under a key or a trait",
);

/// Whether an inert operation belongs to the **framework** rather than to the
/// app, read from the `module_path!()` its decorator recorded.
///
/// A `nest-rs-*` crate is linked as soon as *any* of its capabilities is used,
/// so its opt-in providers — the ones behind a `for_root` the app did not
/// import — are inert in the normal case: `nest_rs_seaorm`'s `db` indicator in
/// an app that imports `SeaOrmDatabaseModule` without `SeaOrmHealthModule`,
/// `nest_rs_authn`'s audience check in every app that does not run a resource
/// server. The developer cannot act on those and they are not mistakes, so they
/// report at `debug`; the app's own inert code stays at `warn`, where the
/// module-gated discovery rule wants it.
///
/// Shared, because it belongs to every discovery site and lived at exactly one:
/// two demo apps warned twice per boot about a `nest-rs-seaorm` indicator —
/// telling the developer to go bind a framework-internal type — while the
/// lifecycle site got the same call right in the same boot.
///
/// **Known limit.** This tests the origin's *crate segment*, so it answers
/// `true` for any crate named `nest_rs_…`, including a third-party plugin that
/// takes the framework's prefix: such a crate's own inert operation is demoted
/// to `debug` and told it is a framework capability. The authoritative answer
/// exists at expansion time — `nest_rs_codegen`'s umbrella resolution already
/// separates "compiled inside the framework" from "compiled against it" — but
/// carrying it here means a new field on all five inventory structs. Recorded
/// as an owner question rather than guessed at more cleverly.
pub fn is_framework_owned(origin: &str) -> bool {
    let krate = origin.split("::").next().unwrap_or(origin);
    krate == "nest_rs" || krate.starts_with("nest_rs_")
}

/// Report a discovered operation whose host the booted container does not hold,
/// at the level its owner earns: `debug` for a `nest-rs-*` capability the app
/// never opted into, `warn` + [`INERT_HOST_HINT`] for the app's own code.
///
/// A macro and not a function, and `tracing` decides that: `target:` and the
/// level land inside a `static` callsite initializer, so both must be const at
/// the call site. Its paths are `$crate::tracing::`, never `::tracing::` — an
/// exported macro's expansion lands in the *caller's* crate and resolves
/// against the caller's extern prelude, so a bare path is an `E0433` inside an
/// expansion nobody can read the day a consumer declares no `tracing`. The
/// kernel's other exported macro was fixed for that reason and wrote it down;
/// this one had kept the bare form. Folding the five targets into one would move four crates'
/// skip lines off the target the observability table assigns them.
///
/// This is the shape [`INERT_HOST_HINT`] and [`is_framework_owned`] were
/// already extracted into, finished: the branch and both sentences lived five
/// times, and the same refactor had been performed at two sites and skipped at
/// three. A new edge owes an inert-entry report, so a sixth site is scheduled
/// rather than hypothetical.
///
/// ```ignore
/// report_inert_host!(
///     target: crate::target::LIFECYCLE,
///     what: "scheduled method",
///     origin: entry.origin,
///     provider = entry.provider,
///     method = entry.method,
/// );
/// ```
#[macro_export]
macro_rules! report_inert_host {
    (target: $target:expr, what: $what:literal, origin: $origin:expr $(, $field:ident = $value:expr)* $(,)?) => {{
        let __origin = $origin;
        if $crate::is_framework_owned(__origin) {
            $crate::tracing::debug!(
                target: $target,
                $($field = $value,)*
                origin = __origin,
                ::core::concat!("skipped ", $what, ": framework capability not imported by this app"),
            );
        } else {
            $crate::tracing::warn!(
                target: $target,
                $($field = $value,)*
                origin = __origin,
                hint = $crate::INERT_HOST_HINT,
                ::core::concat!(
                    "skipped ", $what, ": no instance of the provider in this app's container",
                ),
            );
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hint must name the causes and **prescribe nothing**. Its second
    /// wording offered `providers = [Foo, Foo as dyn Trait]`, which silently
    /// constructs the provider twice — so this pins both halves: the causes are
    /// there, and no imperative that could be followed into that shape is.
    // Which *level* an inert hook is reported at, and why it matters: a `warn`
    // naming a framework-internal provider shows up on every freshly scaffolded
    // auth app (`AudienceBinding`, behind a `OAuthResourceModule` nobody
    // imported), is not actionable, and teaches the reader to ignore a target
    // that also carries security events.
    #[test]
    fn a_framework_owned_origin_is_not_the_developers_problem() {
        for origin in [
            "nest_rs_oauth_resource::module",
            "nest_rs_events::module",
            "nest_rs",
            "nest_rs::authn",
        ] {
            assert!(is_framework_owned(origin), "{origin}");
        }
        // An app's own provider still warns — that is leftover code the
        // developer can act on, and the whole reason the report exists.
        for origin in [
            "features::users::service",
            "api::module",
            "my_nest_rs_helpers::hooks",
        ] {
            assert!(!is_framework_owned(origin), "{origin}");
        }
    }

    /// The known limit, pinned so it is a decision rather than a surprise: a
    /// third-party crate that takes the framework's prefix is read as the
    /// framework's. Found by an audit whose own probe crate was called
    /// `nest-rs-audit-probe` and watched its app-owned hook get demoted.
    #[test]
    fn a_third_party_crate_taking_the_prefix_is_read_as_the_frameworks() {
        assert!(is_framework_owned("nest_rs_audit_probe"));
    }

    /// The hint must name the causes and **prescribe nothing**.
    ///
    /// The first version of this assertion was `!contains("as well")` — a
    /// tripwire on one historic wording rather than on the property. A mutation
    /// test put the same forbidden edit back in different words (*"Also add
    /// `providers = [Foo, Foo as dyn Trait]` …"*) and it passed. So the guard is
    /// on the **shape**: that remedy cannot be written without naming a
    /// `providers` list, and a prescription reads as an imperative.
    /// `listing_a_host_both_ways_builds_it_twice` is what proves the edit must
    /// never be printed.
    #[test]
    fn the_inert_host_hint_names_the_causes_and_prescribes_no_edit() {
        for cause in ["not imported", "dyn Trait", "for_root", "by hand"] {
            assert!(INERT_HOST_HINT.contains(cause), "{INERT_HOST_HINT}");
        }
        assert!(
            !INERT_HOST_HINT.contains("providers = ["),
            "naming a `providers` list is how the double-listing remedy is written, and it \
             constructs the provider twice: {INERT_HOST_HINT}",
        );
        for imperative in [
            "Also add", "also add", "List it", "list it", "as well", "Add `",
        ] {
            assert!(
                !INERT_HOST_HINT.contains(imperative),
                "the hint names causes; only a site that knows which one applies may prescribe \
                 an edit, and this one cannot: {INERT_HOST_HINT}",
            );
        }
    }
}
