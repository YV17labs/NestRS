//! A path poem refuses panics the boot; the macro reads the same grammar, so it
//! is refused at the literal instead.

use nest_rs::http::{controller, routes};

#[controller(path = "/broken")]
struct BrokenController;

#[routes]
impl BrokenController {
    #[get("/items/:")]
    async fn items(&self) -> String {
        "items".into()
    }
}

fn main() {}
