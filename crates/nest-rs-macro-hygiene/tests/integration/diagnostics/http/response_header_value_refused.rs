//! `#[response_header(name, value)]` refuses each position at itself, opening
//! with the decorator and the position: a name that is not a string, a name
//! `HeaderName::from_static` would panic on at boot, an empty one, and a value
//! whose line feed would split the header.

use nest_rs::http::{controller, routes};

#[controller(path = "/a")]
struct AController;

#[routes]
impl AController {
    #[get("/")]
    #[public]
    #[response_header(cache_control, "no-store")]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/b")]
struct BController;

#[routes]
impl BController {
    #[get("/")]
    #[public]
    #[response_header("Cache-Control", "no-store")]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/c")]
struct CController;

#[routes]
impl CController {
    #[get("/")]
    #[public]
    #[response_header("", "no-store")]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/d")]
struct DController;

#[routes]
impl DController {
    #[get("/")]
    #[public]
    #[response_header("x-note", "a\nb")]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
