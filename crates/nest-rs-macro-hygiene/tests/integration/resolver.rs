//! Covers `src/resolver.rs`'s decorators as 7.0.0 placed an operation's
//! attributes: on the field `#[operations]` generates. An app's own attribute
//! imported under another name goes there in every build. One it imports under
//! the name `resilient` goes there in a build without the resilience capability,
//! which `just test` runs as `--no-default-features --features graphql`; in a
//! build where any crate enables that capability, `#[operations]` reads it as
//! the framework's decorator, and the other name is the remedy.

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
