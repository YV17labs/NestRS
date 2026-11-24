//! Covers `src/resolver.rs`'s decorators as 7.0.0 placed an operation's
//! attributes: on the field `#[operations]` generates. An app's own attribute
//! imported under another name goes there in every build. One it imports under
//! the name `resilient` goes there in a build without the resilience capability,
//! which `just test` runs as `--no-default-features --features graphql`; in a
//! build where any crate enables that capability, `#[operations]` reads it as
//! the framework's decorator and keeps it on the developer's method, where it
//! fails to compile or applies to that method alone, and the other name is the
//! remedy.

use nest_rs::core::{Scoped, injectable};
use nest_rs::graphql::async_graphql::Context;
use nest_rs::graphql::{operations, resolver};
// A 7.0 program's own attribute macro, imported under a name of its own.
use tracing::instrument as traced;

/// A lookup's audit, built per request: a `Scoped<Audit>` is no argument of the
/// schema, and no `Debug`, so the field leaves it out.
#[injectable(scope = request)]
#[derive(Default)]
struct Audit;

#[resolver]
struct EntryResolver;

#[operations]
impl EntryResolver {
    /// Instrumented as a 7.0 program instruments it: on the field, whose own
    /// parameters `skip` names and which takes no `Scoped<Audit>` — on the
    /// developer's method, which takes one, the instrument does not compile.
    #[query]
    #[public]
    #[traced(skip(self, ctx))]
    async fn entry(&self, ctx: &Context<'_>, audit: Scoped<Audit>, id: i32) -> String {
        drop(audit);
        format!("{} {id}", ctx.field().name())
    }
}

/// The resolver above compiles, the resilience capability on or off: the
/// attribute the app imports under its own name went on the field.
#[test]
fn an_attribute_an_app_imports_under_a_name_of_its_own_goes_on_the_field_in_every_build() {
    assert!(std::any::type_name::<EntryResolver>().ends_with("EntryResolver"));
}

/// The same program, its attribute imported under the decorator's name.
#[cfg(not(feature = "resilience"))]
mod named_resilient {
    use nest_rs::core::Scoped;
    use nest_rs::graphql::async_graphql::Context;
    use nest_rs::graphql::{operations, resolver};
    // A 7.0 program's own attribute macro, named as the resilience capability's
    // decorator is.
    use tracing::instrument as resilient;

    use super::Audit;

    #[resolver]
    struct NamedEntryResolver;

    #[operations]
    impl NamedEntryResolver {
        /// Instrumented as [`super::EntryResolver`]'s field is, under the
        /// decorator's name.
        #[query]
        #[public]
        #[resilient(skip(self, ctx))]
        async fn named_entry(&self, ctx: &Context<'_>, audit: Scoped<Audit>, id: i32) -> String {
            drop(audit);
            format!("{} {id}", ctx.field().name())
        }
    }

    /// The resolver above compiles: the attribute the app names `resilient`
    /// went on the field, where a build with the capability keeps the decorator
    /// of that name on the developer's method.
    #[test]
    fn an_attribute_an_app_names_resilient_goes_on_the_field_without_the_capability() {
        assert!(std::any::type_name::<NamedEntryResolver>().ends_with("NamedEntryResolver"));
    }
}

/// An attribute an app names `resilient` that compiles on both items, its place
/// read off a call of the developer's method alone, which it spans only there.
mod placed {
    use std::sync::{Arc, Mutex, PoisonError};

    use futures_util::FutureExt as _;
    use nest_rs::graphql::{operations, resolver};
    // A 7.0 program's own attribute macro, named as the resilience capability's
    // decorator is, which compiles on the developer's method and on the field.
    use tracing::instrument as resilient;

    #[resolver]
    struct PlacedResolver;

    #[operations]
    impl PlacedResolver {
        /// Spanned under the decorator's name, its `Result` spelled out, as the
        /// capability asks of an operation it reads as decorated.
        #[query]
        #[public]
        #[resilient(name = "placed", skip_all)]
        async fn placed(&self, id: i32) -> Result<String, std::fmt::Error> {
            Ok(format!("entry {id}"))
        }
    }

    /// Notes the name of each span opened while it is the default subscriber.
    struct Opened(Arc<Mutex<Vec<&'static str>>>);

    impl tracing::Subscriber for Opened {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(span.metadata().name());
            tracing::span::Id::from_u64(1)
        }

        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

        fn event(&self, _: &tracing::Event<'_>) {}

        fn enter(&self, _: &tracing::span::Id) {}

        fn exit(&self, _: &tracing::span::Id) {}
    }

    /// With the resilience capability the attribute stays on the developer's
    /// method, whose own call opens its span; without it, it goes on the field,
    /// as 7.0.0 placed it, and that call opens none.
    #[test]
    fn an_attribute_an_app_names_resilient_goes_on_the_method_with_the_capability_alone() {
        let opened = Arc::new(Mutex::new(Vec::new()));
        let answered = tracing::subscriber::with_default(Opened(Arc::clone(&opened)), || {
            PlacedResolver.placed(7).now_or_never()
        });
        assert_eq!(answered, Some(Ok("entry 7".to_owned())));
        let opened = opened
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if cfg!(feature = "resilience") {
            assert_eq!(opened, ["placed"], "on the developer's method");
        } else {
            assert_eq!(opened, Vec::<&str>::new(), "on the field");
        }
    }
}

/// `#[resilient]` under a `#[cfg_attr]`, beside an attribute of the app's own
/// in the same one: `#[operations]` splits the `cfg_attr` by entry, so the
/// decorator stays on the developer's method, whose `self` holds the policy,
/// and the other goes on the field, as 7.0.0 placed it.
#[cfg(feature = "resilience")]
mod conditional {
    use std::sync::Arc;

    use nest_rs::graphql::{operations, resolver};
    use nest_rs::resilience::{ResilienceError, ResiliencePolicy, resilient};
    use tracing::instrument as traced;

    #[derive(Debug)]
    struct UpstreamDown;

    impl std::fmt::Display for UpstreamDown {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("the upstream did not answer")
        }
    }

    impl std::error::Error for UpstreamDown {}

    #[resolver]
    struct ConditionalResolver {
        #[inject]
        upstream: Arc<ResiliencePolicy>,
    }

    #[operations]
    impl ConditionalResolver {
        #[query]
        #[public]
        #[cfg_attr(all(), resilient(upstream, idempotent), traced(skip_all))]
        async fn conditional(&self, id: i32) -> Result<String, ResilienceError<UpstreamDown>> {
            Ok(format!("entry {id}"))
        }

        /// A condition that does not hold leaves the method undecorated: its
        /// body answers the method's own type.
        #[query]
        #[public]
        #[cfg_attr(any(), resilient(upstream))]
        async fn unconditional(&self, id: i32) -> Result<String, ResilienceError<UpstreamDown>> {
            Ok(format!("entry {id}"))
        }
    }

    /// The resolver above compiles: each entry of the `cfg_attr` went where
    /// it would have gone written plain.
    #[test]
    fn a_resilient_inside_a_cfg_attr_stays_on_the_method_and_its_neighbour_goes_on_the_field() {
        assert!(std::any::type_name::<ConditionalResolver>().ends_with("ConditionalResolver"));
    }
}
