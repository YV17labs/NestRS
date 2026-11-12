//! A pipe on a route reads what an extractor carries, so its carrier names the
//! extractor: `Piped<P>`, the form of the edges that decode their argument
//! themselves, is refused with the spelling a route takes.

use nest_rs::http::{controller, routes};
use nest_rs::pipes::{Piped, Trim};

#[controller(path = "/names")]
struct NamesController;

#[routes]
impl NamesController {
    #[post("/")]
    #[public]
    async fn create(&self, Piped(name): Piped<Trim>) -> String {
        name
    }
}

fn main() {}
