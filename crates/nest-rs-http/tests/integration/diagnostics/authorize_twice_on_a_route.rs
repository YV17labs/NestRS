//! Two `#[authorize(...)]` on one route is refused, not resolved.
//!
//! The refusal keeping two class gates from being written and one silently
//! dropped, and it was unpinned at all three of its sites — HTTP, GraphQL and
//! the shared `PostureRules`. One sentence
//! (`nest_rs_codegen::at_most_one_authorize`), one noun apart per site.

use nest_rs_http::{controller, routes};

struct Read;
struct Update;
mod users {
    pub struct Entity;
}

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    #[authorize(Read, users::Entity)]
    #[authorize(Update, users::Entity)]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
