//! `#[http_code]` takes a status code, and anything else is refused in one
//! sentence opening with the decorator: a string, a number outside the
//! three-digit range, one too large for a `u16`, and no argument at all. Two of
//! these used to be syn's own sentences and two said "expects".

use nest_rs_http::{controller, routes};

#[controller(path = "/a")]
struct AController;

#[routes]
impl AController {
    #[post("/")]
    #[public]
    #[http_code("201")]
    async fn create(&self) -> String {
        String::new()
    }
}

#[controller(path = "/b")]
struct BController;

#[routes]
impl BController {
    #[post("/")]
    #[public]
    #[http_code(42)]
    async fn create(&self) -> String {
        String::new()
    }
}

#[controller(path = "/c")]
struct CController;

#[routes]
impl CController {
    #[post("/")]
    #[public]
    #[http_code(70000)]
    async fn create(&self) -> String {
        String::new()
    }
}

#[controller(path = "/d")]
struct DController;

#[routes]
impl DController {
    #[post("/")]
    #[public]
    #[http_code]
    async fn create(&self) -> String {
        String::new()
    }
}

fn main() {}
