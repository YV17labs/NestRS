use async_graphql::{
    http::{playground_source, GraphQLPlaygroundConfig},
    EmptySubscription, MergedObject, Schema,
};
use nestrs_core::Container;
use poem::{handler, web::Html};

use crate::users::{UsersMutation, UsersQuery};

/// Root GraphQL query — merges every feature query into a single type
/// using `MergedObject`. Add new feature queries here as the API grows.
#[derive(MergedObject, Default)]
pub struct Query(UsersQuery);

/// Root GraphQL mutation — merges every feature mutation.
#[derive(MergedObject, Default)]
pub struct Mutation(UsersMutation);

pub type AppSchema = Schema<Query, Mutation, EmptySubscription>;

/// Build the GraphQL schema and inject the IoC container so resolvers can
/// resolve their dependencies via `ctx.data::<Container>()`.
pub fn build_schema(container: Container) -> AppSchema {
    Schema::build(Query::default(), Mutation::default(), EmptySubscription)
        .data(container)
        .finish()
}

/// Serve the GraphQL playground at `GET /graphql` for local exploration.
#[handler]
pub async fn playground() -> Html<String> {
    Html(playground_source(GraphQLPlaygroundConfig::new("/graphql")))
}
