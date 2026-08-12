//! `version = "…"` is a declaration on every edge whose mount is an address a
//! client selects, and GraphQL is not one. The refusal has to say *that* — one
//! schema, and `#[graphql(deprecation = …)]` for the field being retired —
//! rather than fall through to `#[resolver]`'s generic "takes no arguments",
//! which reads as a spelling correction for an argument that exists.

use nest_rs_graphql::resolver;

#[resolver(version = "1")]
struct DemoResolver;

fn main() {}
