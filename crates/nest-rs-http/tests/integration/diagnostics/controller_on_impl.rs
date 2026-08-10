//! The converse: `#[controller]` names the struct only. Reaching for it on the
//! impl block must name the sibling that does belong there — the shape the
//! developer reached for exists, it is just spelled `#[routes]`.

use nest_rs_http::controller;

struct DemoController;

#[controller(path = "/demo")]
impl DemoController {
    async fn ping(&self) {}
}

fn main() {}
