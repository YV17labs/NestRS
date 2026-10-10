//! 6.x read `:id` with poem's grammar; 7.0 reads a route path as OpenAPI path
//! templating, so the old spelling is refused with the new one written out.

use nest_rs::http::{controller, routes};

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/:id/posts")]
    async fn posts(&self) -> String {
        String::new()
    }
}

fn main() {}
