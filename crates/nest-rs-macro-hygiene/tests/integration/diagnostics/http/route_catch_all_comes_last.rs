//! A catch-all takes the rest of the path, so nothing follows it.

use nest_rs::http::{controller, routes};

#[controller(path = "/files")]
struct FilesController;

#[routes]
impl FilesController {
    #[get("/{*path}/meta")]
    async fn meta(&self) -> String {
        String::new()
    }
}

fn main() {}
