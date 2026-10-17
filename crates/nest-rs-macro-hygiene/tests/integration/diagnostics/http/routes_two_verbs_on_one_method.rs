//! A `#[routes]` method declares one verb. Two used to mount the first and leave
//! the second on the method, where rustc called it an unknown attribute; the
//! family's refusal now names both, with the caret on the second.

use nest_rs::http::{controller, routes};

#[controller(path = "/demo")]
struct DemoController;

#[routes]
impl DemoController {
    #[get("/")]
    #[post("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
