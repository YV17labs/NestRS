//! An unnarrowed route beside a narrowed one under conditions the macro cannot
//! evaluate: the pair is refused by the compiler wherever both are compiled.

use nest_rs::http::{controller, routes};

#[controller(path = "/every", version = ["1", "2"])]
struct EveryController;

#[routes]
impl EveryController {
    #[cfg(all())]
    #[get("/v")]
    async fn every(&self) -> String {
        "every".into()
    }

    #[cfg(not(any()))]
    #[get("/v")]
    #[version("2")]
    async fn second(&self) -> String {
        "second".into()
    }
}

fn main() {}
