//! A version listed twice declares one address twice, so the list is refused
//! where it is written. The twin of `controller_declares_version_twice`, which
//! refuses the key written twice: this is the same question asked inside the
//! list.

use nest_rs::http::{controller, routes};

#[controller(path = "/reports", version = ["1", "1"])]
struct ReportsController;

#[routes]
impl ReportsController {
    #[get("/")]
    async fn list(&self) -> String {
        "list".into()
    }
}

fn main() {}
