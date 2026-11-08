//! `unmasked` is refused **by name** on every edge, with the fact that makes it
//! meaningless: no reply mask is left to switch off, since each wire row a reply
//! carries was judged for its caller where `#[expose]`'s conversion built it.

use nest_rs::http::{controller, routes};

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
