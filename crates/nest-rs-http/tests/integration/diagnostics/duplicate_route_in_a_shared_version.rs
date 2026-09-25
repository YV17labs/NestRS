//! A route is claimed in the versions it serves. Two narrowed routes sharing a
//! version, or an unnarrowed route beside a narrowed one, answer one address.

use nest_rs_http::{controller, routes};

#[controller(path = "/shared", version = ["1", "2"])]
struct SharedController;

#[routes]
impl SharedController {
    #[get("/v")]
    #[version("1", "2")]
    async fn both(&self) -> String {
        "both".into()
    }

    #[get("/v")]
    #[version("2")]
    async fn second(&self) -> String {
        "second".into()
    }
}

#[controller(path = "/every", version = ["1", "2"])]
struct EveryController;

#[routes]
impl EveryController {
    #[get("/v")]
    async fn every(&self) -> String {
        "every".into()
    }

    #[get("/v")]
    #[version("2")]
    async fn second(&self) -> String {
        "second".into()
    }
}

fn main() {}
