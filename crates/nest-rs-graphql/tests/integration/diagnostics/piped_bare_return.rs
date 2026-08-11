//! A pipe rejects, and the rejection has to reach the client — so an operation
//! taking a `Piped<P, T>` returns `Result<...>`. Without the named error this
//! surfaced as "cannot use the `?` operator", pointing at `#[operations]`.

use nest_rs_graphql::{operations, resolver};
// The failed expansion leaves the method out, so these read as unused.
#[allow(unused_imports)]
use nest_rs_pipes::{Piped, Trim};

#[resolver]
struct DemoResolver;

#[operations]
impl DemoResolver {
    #[query]
    #[public]
    async fn shout(&self, label: Piped<Trim, String>) -> String {
        label.into_inner()
    }
}

fn main() {}
