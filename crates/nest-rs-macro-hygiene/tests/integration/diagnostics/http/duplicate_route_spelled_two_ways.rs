//! A route's identity is the address poem mounts, not the path as written: a
//! parameter's name, a missing leading slash and a trailing slash the edge trims
//! all leave the address where it was, so each pair below is one route twice.

use nest_rs::http::{controller, routes};

#[controller(path = "/named")]
struct NamedController;

#[routes]
impl NamedController {
    #[get("/q/:id")]
    async fn first(&self) -> String {
        "first".into()
    }

    #[get("/q/:other")]
    async fn second(&self) -> String {
        "second".into()
    }
}

#[controller(path = "/slashed")]
struct SlashedController;

#[routes]
impl SlashedController {
    #[get("/u")]
    async fn first(&self) -> String {
        "first".into()
    }

    #[get("/u/")]
    async fn second(&self) -> String {
        "second".into()
    }
}

#[controller(path = "/rooted")]
struct RootedController;

#[routes]
impl RootedController {
    #[get("/t")]
    async fn first(&self) -> String {
        "first".into()
    }

    #[get("t")]
    async fn second(&self) -> String {
        "second".into()
    }
}

fn main() {}
