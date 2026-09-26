//! `#[api]`'s values, each of the wrong kind: a tag that is not a string, `tags`
//! given no list, and a type-valued key given a literal. Each opens with its
//! site; syn named none of them, and for a type answered with the fifteen tokens
//! a type may start with.

use nest_rs_http::{controller, routes};

#[controller(path = "/a")]
struct AController;

#[routes]
impl AController {
    #[get("/")]
    #[public]
    #[api(tags(users))]
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
    #[api(tags = "users")]
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
    #[api(response = "Post")]
    async fn list(&self) -> String {
        String::new()
    }
}

#[controller(path = "/d")]
struct DController;

#[routes]
impl DController {
    #[post("/")]
    #[public]
    #[api(multipart = 42)]
    async fn upload(&self) -> String {
        String::new()
    }
}

fn main() {}
