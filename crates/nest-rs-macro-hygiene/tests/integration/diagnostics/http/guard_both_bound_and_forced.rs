//! A guard listed by both `#[use_guards]` and `#[force_guards]` is refused:
//! `#[force_guards]` alone binds it at the method and re-runs it, so the second
//! list says nothing.

use nest_rs::http::{controller, routes};

struct FreshToken;

#[controller(path = "/widgets")]
struct WidgetsController;

#[routes]
impl WidgetsController {
    #[get("/")]
    #[public]
    #[use_guards(FreshToken)]
    #[force_guards(FreshToken)]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
