//! A transport calls a handler with what the request carries and nothing else,
//! so a type parameter has nothing to be inferred from.

use nest_rs_http::{controller, routes};

#[controller(path = "/generic")]
struct GenericController;

#[routes]
impl GenericController {
    #[get("/")]
    async fn generic<T: Default + ToString>(&self) -> String {
        T::default().to_string()
    }
}

fn main() {}
