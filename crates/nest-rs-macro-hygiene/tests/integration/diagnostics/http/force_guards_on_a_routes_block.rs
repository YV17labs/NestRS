//! A host cell the table refuses, written on the impl half, names the block it
//! is written on: the attribute sits on `#[routes]`, not beside `#[controller]`.

use nest_rs::http::{controller, routes};

struct Fresh;

#[controller(path = "/widgets")]
struct WidgetsController;

#[routes]
#[force_guards(Fresh)]
impl WidgetsController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
