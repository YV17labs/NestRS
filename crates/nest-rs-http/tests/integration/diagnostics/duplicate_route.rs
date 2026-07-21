//! Two handlers for the same (verb, path) would collapse silently into
//! `poem::get(h1).get(h2)` — the macro must reject the duplicate (HTTP-R2).

use nest_rs_http::{controller, routes};

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
