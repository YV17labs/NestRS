//! Covers `src/resolver.rs`'s decorators in a build without the resilience
//! capability, which `just test` runs as `--no-default-features --features
//! graphql`: every attribute of an operation goes on the field `#[operations]`
//! generates, as 7.0.0 placed them, one an app imports under the name
//! `resilient` included.

use nest_rs::core::{Scoped, injectable};
use nest_rs::graphql::async_graphql::Context;
use nest_rs::graphql::{operations, resolver};
// A 7.0 program's own attribute macro, named as the resilience capability's
// decorator is.
use tracing::instrument as resilient;

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
    #[resilient(skip(self, ctx))]
    async fn entry(&self, ctx: &Context<'_>, audit: Scoped<Audit>, id: i32) -> String {
        drop(audit);
        format!("{} {id}", ctx.field().name())
    }
}

/// The resolver above compiles: the attribute the app names `resilient` went on
/// the field, where a build with the capability would keep the decorator of that
/// name on the developer's method.
#[test]
fn an_attribute_an_app_names_resilient_goes_on_the_field_without_the_capability() {
    assert!(std::any::type_name::<EntryResolver>().ends_with("EntryResolver"));
}
