//! The length bound lived only on the wire side, so this compiled, mounted,
//! logged and documented — and was then refused with `400` the moment a caller
//! named it. Both halves now read one grammar.

use nest_rs_http::{controller, routes};

#[controller(path = "/reports", version = "0123456789012345678901234567890123456789")]
struct ReportsController;

#[routes]
impl ReportsController {
    #[get("/")]
    async fn list(&self) -> String {
        "list".into()
    }
}

fn main() {}
