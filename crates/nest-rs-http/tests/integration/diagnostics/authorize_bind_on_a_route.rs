//! `bind = Service` is refused **by name** on HTTP, with the fact that makes it
//! GraphQL's alone.
//!
//! It answered syn's `` expected `,` `` — the parse died on the `=` before any
//! arm saw the key — so the developer got neither the key nor the reason, and
//! the remedy sentence they needed sat one branch away, unreachable for the only
//! input it was written for. Its sibling `unmasked` had already been lifted out
//! of `PostureRules` for exactly this; `bind` was left behind.

use nest_rs_http::{controller, routes};

struct Update;
struct UsersService;
mod users {
    pub struct Entity;
}

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    #[authorize(Update, bind = UsersService)]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
