//! The host half declares construction and provider-scope layers, and nothing
//! else — no path, no version, no address of any kind, because a schema has
//! none. Its sibling `#[operations]` carries the same refusal; both were
//! correct and neither was pinned, which is how the HTTP and WS pairs came to
//! have a snapshot each and this one none.

use nest_rs_graphql::resolver;

#[resolver(path = "/graphql")]
struct DemoResolver;

fn main() {}
