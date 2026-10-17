//! A parameter reads up to the next `/`, so text after it in its segment is
//! refused: the next router does not route it.

use nest_rs::http::{controller, routes};

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/{id}.json")]
    async fn read(&self) -> String {
        String::new()
    }
}

fn main() {}
