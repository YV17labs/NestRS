//! A handler with no `#[cfg]` and one whose `#[cfg]` holds are two handlers for
//! one verb and path. The conditions differ as written and agree as evaluated,
//! and only rustc evaluates them — so rustc refuses the pair.

use nest_rs_http::{controller, routes};

#[controller(path = "/dup")]
struct DupController;

#[routes]
impl DupController {
    #[get("/")]
    async fn first(&self) -> String {
        "first".into()
    }

    #[cfg(all())]
    #[get("/")]
    async fn second(&self) -> String {
        "second".into()
    }
}

fn main() {}
