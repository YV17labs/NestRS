//! `unmasked` is refused **by name** on HTTP, with the fact that makes it
//! impossible here.
//!
//! It is shared posture vocabulary at the three in-band edges (`#[tools]`,
//! `#[messages]`, `#[operations]`), so a developer who learned it on one reaches
//! for it on the fourth. HTTP answered with a shape error naming neither the key
//! nor the reason — which is the silence `CLAUDE.md`'s *One declaration, every
//! site the standard permits* forbids: a site that cannot follow owes a
//! sentence, not a stub.

use nest_rs_http::{controller, routes};

struct Read;
mod users {
    pub struct Entity;
}

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    #[authorize(Read, users::Entity, unmasked)]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
