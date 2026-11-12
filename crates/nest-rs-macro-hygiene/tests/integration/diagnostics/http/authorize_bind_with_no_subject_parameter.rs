//! `bind = Service` loads a subject, and a route that takes no
//! `Authorized<Action, E>` parameter has nowhere to put it: the refusal names
//! the parameter to add, in the sentence `#[operations]` prints.

use nest_rs::http::{controller, routes};

struct Update;
struct UsersService;

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[post("/{id}/archive")]
    #[authorize(Update, bind = UsersService)]
    async fn archive(&self) -> String {
        String::new()
    }
}

fn main() {}
