//! A host-scope layer on the impl half. Refused through the one
//! `DecoratorPair::reject_host_layers` sentence every edge takes, so the four
//! cannot drift — two used to word it themselves and had already disagreed
//! about which siblings the uniformity spanned, and the other two said nothing
//! at all: `use_guards` is no standalone attribute macro, so it reached rustc
//! as `cannot find attribute` with no transport, reason or remedy named.

use nest_rs_http::{controller, routes};

struct AllowAll;

#[controller(path = "/widgets")]
struct WidgetsController;

#[routes]
#[use_guards(AllowAll)]
impl WidgetsController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
