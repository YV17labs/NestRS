//! `#[routes]` reads a pipe carrier by its spelling, where it holds the source
//! to the rules its type declares before the pipe runs. `Piped` is no
//! extractor, so a carrier behind an alias is refused rather than read without
//! those rules.

use nest_rs::http::{Json, controller, routes};
use nest_rs::pipes::{Piped, Trim};

type Trimmed = Piped<Trim, Json<String>>;

#[controller(path = "/names")]
struct NamesController;

#[routes]
impl NamesController {
    #[post("/")]
    #[public]
    async fn create(&self, Piped(name): Trimmed) -> String {
        name
    }
}

fn main() {}
