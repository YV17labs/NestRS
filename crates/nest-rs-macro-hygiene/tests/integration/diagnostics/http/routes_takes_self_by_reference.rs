//! A `#[routes]` method borrows its controller. One with no receiver is refused
//! with that fact — it used to have its first real argument skipped as though it
//! were `&self`, and the extractor it named went missing from the route.

use nest_rs::http::{controller, routes};

#[controller(path = "/demo")]
struct DemoController;

#[routes]
impl DemoController {
    #[get("/:id")]
    #[public]
    async fn show(id: nest_rs::http::poem::web::Path<String>) -> String {
        id.0
    }
}

fn main() {}
