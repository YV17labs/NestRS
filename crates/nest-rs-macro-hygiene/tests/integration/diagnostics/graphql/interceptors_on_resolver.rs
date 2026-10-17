//! `#[use_interceptors]` on a resolver would be a silent no-op (GraphQL does
//! not run interceptors yet) — the macro rejects it and points to the HTTP
//! home where the layer runs.

use nest_rs::graphql::resolver;

#[resolver]
#[use_interceptors(SomeInterceptor)]
struct BadResolver;

fn main() {}
