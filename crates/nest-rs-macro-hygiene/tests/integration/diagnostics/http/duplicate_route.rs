//! Two handlers for the same (verb, path) would leave one of them unreachable —
//! the macro must reject the duplicate (HTTP-R2).

use nest_rs::http::{controller, routes};

#[controller(path = "/dup")]
struct DupController;

#[routes]
impl DupController {
    #[get("/")]
    async fn first(&self) -> String {
        "first".into()
    }

    #[get("/")]
    async fn second(&self) -> String {
        "second".into()
    }
}

fn main() {}
