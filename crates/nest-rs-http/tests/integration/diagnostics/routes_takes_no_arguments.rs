//! The impl half collects; it declares nothing. `#[routes]` took an argument
//! list and dropped it — including `version`, which the `#[controller]` one line
//! up genuinely declares, making this the likeliest place of all to reach for it
//! and get silence.

use nest_rs_http::{controller, routes};

#[controller(path = "/reports")]
struct ReportsController;

#[routes(version = "2")]
impl ReportsController {
    #[get("/")]
    async fn list(&self) -> String {
        "list".into()
    }
}

fn main() {}
