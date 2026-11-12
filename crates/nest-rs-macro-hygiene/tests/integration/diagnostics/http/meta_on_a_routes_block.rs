//! A value for every route of a controller is declared on the struct, beside
//! `#[controller]`; the `#[routes]` block declares the routes.

use nest_rs::http::{controller, routes};

struct Limit(u32);

#[controller(path = "/limits")]
struct LimitsController;

#[routes]
#[meta(Limit(10))]
impl LimitsController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
