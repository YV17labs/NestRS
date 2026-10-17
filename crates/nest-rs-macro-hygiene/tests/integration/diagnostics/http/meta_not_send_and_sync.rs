//! A `#[meta]` value is shared by every request its route serves, so it is
//! `Send + Sync`: an `Rc` is refused where it is written, naming `Send`.

use nest_rs::http::{controller, routes};

#[controller(path = "/limits")]
struct LimitsController;

#[routes]
impl LimitsController {
    #[get("/")]
    #[public]
    #[meta(std::rc::Rc::new(1_u32))]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
