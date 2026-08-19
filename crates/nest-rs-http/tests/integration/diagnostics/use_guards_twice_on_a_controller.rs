//! Two `#[use_guards(...)]` on one host is refused, not merged.
//!
//! `nest_rs_codegen::take_path_list` words this for eight call sites across
//! `#[controller]`, `#[routes]`, `#[gateway]`, `#[messages]`, `#[resolver]` and
//! `#[mcp]`, and nothing pinned it: a second list silently dropping half a guard
//! chain is what it refuses, and the refusal could have gone without a suite
//! noticing.

use nest_rs_http::{controller, routes};

struct FirstGuard;
struct SecondGuard;

#[controller(path = "/users")]
#[use_guards(FirstGuard)]
#[use_guards(SecondGuard)]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
