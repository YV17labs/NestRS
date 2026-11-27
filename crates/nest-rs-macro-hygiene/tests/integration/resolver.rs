//! Covers `src/resolver.rs`'s decorators where an operation carries an
//! attribute of the app's own: `#[operations]` leaves it on the developer's
//! method, as written, whatever it is named — the resilience decorator's name
//! included, in a build with that capability or without.

use nest_rs::core::{Scoped, injectable};
use nest_rs::graphql::async_graphql::Context;
use nest_rs::graphql::{operations, resolver};
// The app's own attribute macro, imported under a name of its own.
use tracing::instrument as traced;

/// A lookup's audit, built per request: a `Scoped<Audit>` is no argument of the
/// schema, and no `Debug`, so an instrument skips it.
#[injectable(scope = request)]
#[derive(Default)]
struct Audit;

#[resolver]
#[derive(Default)]
struct EntryResolver {
    shelf: u32,
}

#[operations]
impl EntryResolver {
    /// Instrumented as a route is: the span is the method's, reading its own
    /// `self` and its own parameters.
    #[query]
    #[public]
    #[traced(skip(self, ctx, audit), fields(shelf = self.shelf))]
    async fn entry(&self, ctx: &Context<'_>, audit: Scoped<Audit>, id: i32) -> String {
        drop(audit);
        format!("{} {id}", ctx.field().name())
    }
}

/// Notes the name of each span opened while it is the default subscriber.
struct Opened(std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);

impl tracing::Subscriber for Opened {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(span.metadata().name());
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, _: &tracing::Event<'_>) {}

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// The names of the spans `call` opens.
fn opened_by<T>(call: impl FnOnce() -> T) -> (T, Vec<&'static str>) {
    let opened = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let answered = tracing::subscriber::with_default(Opened(std::sync::Arc::clone(&opened)), call);
    let opened = opened
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    (answered, opened)
}

/// An attribute an app names as the resilience capability's decorator is named
/// is the app's own: on an operation answering no `Result`, it compiles and
/// spans the method it was written on.
mod named_resilient {
    use futures_util::FutureExt as _;
    use nest_rs::graphql::{operations, resolver};
    // The app's own attribute macro, named as the decorator is.
    use tracing::instrument as resilient;

    #[resolver]
    struct NamedEntryResolver;

    #[operations]
    impl NamedEntryResolver {
        #[query]
        #[public]
        #[resilient(name = "named_entry", skip_all)]
        async fn entry(&self, id: i32) -> String {
            format!("entry {id}")
        }
    }

    /// A call of the developer's method opens the span: the attribute stayed
    /// on it, in every build.
    #[test]
    fn an_attribute_an_app_names_resilient_stays_on_the_method_in_every_build() {
        let (answered, opened) = super::opened_by(|| NamedEntryResolver.entry(7).now_or_never());
        assert_eq!(answered.as_deref(), Some("entry 7"));
        assert_eq!(opened, ["named_entry"]);
    }
}

/// `#[resilient]` under a `#[cfg_attr]`, beside an attribute of the app's own
/// in the same one: `#[operations]` leaves the whole `cfg_attr` on the
/// developer's method, whose `self` holds the policy.
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
    }

    /// The resolver above compiles: both entries of the `cfg_attr` stayed on
    /// the method, the decorator reading the policy off its `self`.
    #[test]
    fn a_cfg_attr_holding_the_decorator_and_an_instrument_stays_on_the_method() {
        assert!(std::any::type_name::<ConditionalResolver>().ends_with("ConditionalResolver"));
    }
}

/// The resolver at the top compiles: its instrument names the method's own
/// `self` and its `Scoped<Audit>`, which the field `#[operations]` generates
/// has neither of.
#[test]
fn an_attribute_an_app_imports_under_a_name_of_its_own_stays_on_the_method() {
    assert!(std::any::type_name::<EntryResolver>().ends_with("EntryResolver"));
}
