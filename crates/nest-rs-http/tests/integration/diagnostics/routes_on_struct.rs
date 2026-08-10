//! One decorator, one item shape: `#[routes]` collects a controller's verbs, so
//! on the struct it must point back at `#[controller]` rather than report the
//! shape it happened to expect.

use nest_rs_http::routes;

#[routes]
struct DemoController;

fn main() {}
