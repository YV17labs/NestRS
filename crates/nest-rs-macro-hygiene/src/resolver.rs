//! `#[resolver]` + `#[operations]`, through the umbrella alone.
//!
//! `#[operations]` wraps async-graphql's `#[Object]`, which roots its paths at
//! the call site's manifest, falling back to a bare `::async_graphql`; this
//! compile fails if the `crate = ` override is ever dropped.

use nest_rs::graphql::async_graphql::SimpleObject;
use nest_rs::graphql::async_graphql::futures_util::stream::{self, Stream};
use nest_rs::graphql::{operations, resolver};

/// The federated type; its `@key` comes from the entity resolver's arguments.
#[derive(SimpleObject)]
#[graphql(crate = "::nest_rs::graphql::async_graphql")]
pub struct Greeting {
    pub id: i32,
    pub text: String,
}

/// The lead snippet of `/graphql/`, verbatim.
#[resolver]
pub struct GreetingResolver;

#[operations]
impl GreetingResolver {
    #[query]
    #[public]
    async fn greeting(&self, name: Option<String>) -> String {
        format!("Hello, {}!", name.as_deref().unwrap_or("World"))
    }

    #[subscription]
    #[public]
    async fn greetings(&self) -> impl Stream<Item = String> {
        stream::iter(["Hello, World!".to_string()])
    }

    #[entity]
    #[public]
    async fn find_greeting_by_id(
        &self,
        id: i32,
    ) -> nest_rs::graphql::async_graphql::Result<Greeting> {
        Ok(Greeting {
            id,
            text: "Hello, World!".to_owned(),
        })
    }

    #[query]
    #[public]
    fn farewell(&self) -> String {
        "Goodbye!".to_owned()
    }

    #[query]
    #[public]
    fn steady(&self) -> Result<String, crate::never::Never> {
        Ok("steady".to_owned())
    }

    #[cfg(feature = "seaorm")]
    #[query]
    #[authorize(nest_rs::authz::Read, crate::entity::Entity)]
    fn note_count(&self) -> u64 {
        0
    }

    /// An operation compiled out takes its root field and guard with it.
    #[cfg(any())]
    #[query]
    #[public]
    #[use_guards(crate::does_not_exist::Guard)]
    async fn compiled_out(
        &self,
        input: crate::does_not_exist::Input,
    ) -> crate::does_not_exist::Output {
        crate::does_not_exist::answer(input)
    }

    /// async-graphql's `#[Object]` reads a plain `#[cfg]` only, so a
    /// `#[cfg_attr]` must reach it unfolded.
    #[cfg_attr(all(), cfg(any()))]
    #[query]
    #[public]
    fn compiled_out_by_cfg_attr(&self) -> crate::does_not_exist::Output {
        crate::does_not_exist::answer()
    }

    #[subscription]
    #[public]
    async fn farewells(&self) -> impl Stream<Item = String> {
        stream::iter(["Goodbye!".to_string()])
    }

    /// A duplicate operation name under an excluding condition is not a duplicate field.
    #[cfg(any())]
    #[query]
    #[public]
    fn farewell_elsewhere(&self) -> String {
        "Goodbye!".to_owned()
    }
}

#[derive(SimpleObject)]
#[graphql(complex, crate = "::nest_rs::graphql::async_graphql")]
pub struct Parcel {
    pub id: i32,
}

#[resolver]
pub struct ParcelResolver;

#[operations]
impl ParcelResolver {
    #[query]
    #[public]
    fn parcel(&self) -> Parcel {
        Parcel { id: 1 }
    }

    /// A field resolver compiled out through a `#[cfg_attr]`, likewise.
    #[cfg_attr(all(), cfg(any()))]
    #[field_resolver]
    fn gone(&self, parent: &Parcel) -> crate::does_not_exist::Output {
        crate::does_not_exist::answer(parent)
    }
}
