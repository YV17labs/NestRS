//! `#[redirect(url[, status])]` refuses each position at itself, opening with
//! the decorator and the position: a URL that is not a string, one missing, a
//! URL byte the `Location` header cannot carry, and a status that is not a
//! redirect — as a number and as a string.

use nest_rs::http::{controller, routes};

#[controller(path = "/a")]
struct AController;

#[routes]
impl AController {
    #[get("/")]
    #[public]
    #[redirect(login)]
    async fn go(&self) {}
}

#[controller(path = "/b")]
struct BController;

#[routes]
impl BController {
    #[get("/")]
    #[public]
    #[redirect]
    async fn go(&self) {}
}

#[controller(path = "/c")]
struct CController;

#[routes]
impl CController {
    #[get("/")]
    #[public]
    #[redirect("/log in")]
    async fn go(&self) {}
}

#[controller(path = "/d")]
struct DController;

#[routes]
impl DController {
    #[get("/")]
    #[public]
    #[redirect("/login", 200)]
    async fn go(&self) {}
}

#[controller(path = "/e")]
struct EController;

#[routes]
impl EController {
    #[get("/")]
    #[public]
    #[redirect("/login", "301")]
    async fn go(&self) {}
}

fn main() {}
