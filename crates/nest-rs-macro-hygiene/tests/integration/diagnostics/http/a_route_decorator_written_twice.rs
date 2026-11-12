//! A decorator read once is refused by name when written twice. The first copy
//! was taken and the second left on the method, where rustc reported
//! `cannot find attribute` — no decorator, no reason, no remedy. One sentence
//! for every such attribute (`nest_rs::codegen::take_single_attr`), pinned at
//! each edge.

use nest_rs::http::{controller, routes};

#[controller(path = "/demo")]
struct DemoController;

#[routes]
impl DemoController {
    #[get("/ping")]
    #[public]
    #[public]
    async fn ping(&self) -> String {
        "pong".to_owned()
    }
}

#[controller(path = "/docs")]
struct DocsController;

#[routes]
impl DocsController {
    #[get("/documented")]
    #[public]
    #[api(summary = "first")]
    #[api(summary = "second")]
    async fn documented(&self) -> String {
        "documented".to_owned()
    }
}

fn main() {}
