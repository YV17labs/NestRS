//! A layer list names types, and an entry that is not a type path is refused as
//! a value, opening with the attribute — worded once in `take_path_list` for
//! every `#[use_*]` list at every edge. syn's `expected identifier` named
//! neither the attribute nor what it lists.

use nest_rs_http::{controller, routes};

#[controller(path = "/a")]
#[use_guards("SessionGuard")]
struct AController;

#[routes]
impl AController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/b")]
#[use_exception_filters(42)]
struct BController;

#[routes]
impl BController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
