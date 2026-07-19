//! `#[use_interceptors]` on a resolver would be a silent no-op (interceptors
//! are not bridged on GraphQL) — the macro rejects it and points to the HTTP
//! home where the layer actually runs.

use nest_rs_graphql::resolver;

#[resolver]
#[use_interceptors(SomeInterceptor)]
struct BadResolver;

fn main() {}
