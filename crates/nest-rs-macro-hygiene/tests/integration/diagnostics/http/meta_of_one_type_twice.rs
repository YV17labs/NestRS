//! Two `#[meta]` values of one type on one route: a read takes the first of a
//! type, so the second would be read by nothing. Refused at the second.

use nest_rs::http::{controller, routes};

struct Limit(u32);

#[controller(path = "/limits")]
struct LimitsController;

#[routes]
impl LimitsController {
    #[get("/")]
    #[public]
    #[meta(Limit(10))]
    #[meta(Limit { 0: 20 })]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
