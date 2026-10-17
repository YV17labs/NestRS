//! A route's `#[version(...)]` is the version grammar written positionally, so
//! its refusals name the attribute alone — it has no `version` key, and they
//! used to cite one — and a version that is not a string reaches the same
//! sentence rather than syn's `expected string literal`.

use nest_rs::http::{controller, routes};

#[controller(path = "/a", version = ["1", "2"])]
struct AController;

#[routes]
impl AController {
    #[get("/")]
    #[public]
    #[version(2)]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/b", version = ["1", "2"])]
struct BController;

#[routes]
impl BController {
    #[get("/")]
    #[public]
    #[version("2 beta")]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/c", version = ["1", "2"])]
struct CController;

#[routes]
impl CController {
    #[get("/")]
    #[public]
    #[version("1", "1")]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/d", version = ["1", "2"])]
struct DController;

#[routes]
impl DController {
    #[get("/")]
    #[public]
    #[version()]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
