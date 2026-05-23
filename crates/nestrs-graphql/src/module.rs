//! `GraphqlModule` — import it to serve the auto-discovered schema over HTTP.

use nestrs_core::{Container, ContainerBuilder, Module};
use nestrs_http::HttpEndpointMeta;
use poem::web::Html;
use poem::Route;

use crate::resolver::build_schema;

const GRAPHQL_PATH: &str = "/graphql";

#[poem::handler]
fn playground() -> Html<String> {
    Html(async_graphql::http::playground_source(
        async_graphql::http::GraphQLPlaygroundConfig::new(GRAPHQL_PATH),
    ))
}

/// Add to a `#[module(imports = [...])]` to expose GraphQL over HTTP:
/// `POST /graphql` (queries + mutations) and `GET /graphql` (playground).
///
/// Every `#[resolver]` in the binary self-registers via the link-time
/// registry, so the schema composes itself — there is nothing else to wire,
/// no central resolver list, no `main.rs` mount. This is the GraphQL analog of
/// NestJS's `GraphQLModule.forRoot()`.
pub struct GraphqlModule;

impl Module for GraphqlModule {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.provide_meta(HttpEndpointMeta::new(
            GRAPHQL_PATH,
            "graphql",
            |container: &Container, route: Route| {
                let schema = build_schema(container.clone());
                route.nest(
                    GRAPHQL_PATH,
                    poem::post(async_graphql_poem::GraphQL::new(schema)).get(playground),
                )
            },
        ))
    }
}
