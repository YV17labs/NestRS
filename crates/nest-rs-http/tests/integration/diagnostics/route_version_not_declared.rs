//! A `#[version]` naming a version the controller never declared would mount
//! nowhere — the transport loops over the *controller's* versions — leaving a
//! handler that compiles, registers, documents itself and answers nothing.
//! `versions_declare` turns that into a compile error at the route.

use nest_rs_http::{controller, routes};

#[controller(path = "/reports", version = ["1", "2"])]
struct ReportsController;

#[routes]
impl ReportsController {
    #[get("/")]
    #[version("3")]
    async fn list(&self) -> String {
        "list".into()
    }
}

fn main() {}
