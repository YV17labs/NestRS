//! 6.x's catch-all was poem's `*rest`; 7.0's is `{*rest}`, and the refusal
//! writes it out.

use nest_rs::http::{controller, routes};

#[controller(path = "/files")]
struct FilesController;

#[routes]
impl FilesController {
    #[get("/*rest")]
    async fn read(&self) -> String {
        String::new()
    }
}

fn main() {}
