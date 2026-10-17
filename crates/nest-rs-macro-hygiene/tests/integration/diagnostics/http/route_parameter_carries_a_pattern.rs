//! A parameter is parsed by its type or a pipe, where a refusal is typed and
//! named: a regular expression in the path is refused in poem's 6.x spelling
//! and in the braces alike.

use nest_rs::http::{controller, routes};

#[controller(path = "/legacy")]
struct LegacyController;

#[routes]
impl LegacyController {
    #[get("/:id<\\d+>")]
    async fn read(&self) -> String {
        String::new()
    }
}

#[controller(path = "/braced")]
struct BracedController;

#[routes]
impl BracedController {
    #[get("/{id:\\d+}")]
    async fn read(&self) -> String {
        String::new()
    }
}

fn main() {}
