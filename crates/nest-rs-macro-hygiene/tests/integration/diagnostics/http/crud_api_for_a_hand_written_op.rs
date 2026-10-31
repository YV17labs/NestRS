//! `api(…)` documents an operation `#[crud]` generates; beside a method the
//! block writes by hand, nothing is generated for it to document — refused as
//! `paginate` beside a hand-written `list` is, never a doc dropped without a
//! word.

use nest_rs::http::{controller, crud};

#[controller(path = "/orgs")]
struct OrgsController;

#[crud(
    service = OrgsService,
    ops = [list, get],
    api(get(summary = "Get an org by id")),
)]
impl OrgsController {
    #[get("/{id}")]
    #[public]
    async fn get(&self) -> String {
        String::new()
    }
}

fn main() {}
