//! A declared version is spliced straight into a URL path, so the character set
//! is checked where it is written rather than surfacing as a route nobody can
//! call. The wire-side validator refuses the same set at runtime; this is the
//! half that catches your own typo.

use nest_rs_http::{controller, routes};

#[controller(path = "/reports", version = "1/2")]
struct ReportsController;

#[routes]
impl ReportsController {
    #[get("/")]
    async fn list(&self) -> String {
        "list".into()
    }
}

fn main() {}
