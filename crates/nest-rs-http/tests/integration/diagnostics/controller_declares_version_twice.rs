//! A repeated key was a plain assignment, so the last one silently won and the
//! controller mounted at an address nobody wrote. `version = ["1", "1"]` was
//! already refused — the same question, asked in the other spelling.

use nest_rs_http::{controller, routes};

#[controller(path = "/reports", version = "1", version = "2")]
struct ReportsController;

#[routes]
impl ReportsController {
    #[get("/")]
    async fn list(&self) -> String {
        "list".into()
    }
}

fn main() {}
