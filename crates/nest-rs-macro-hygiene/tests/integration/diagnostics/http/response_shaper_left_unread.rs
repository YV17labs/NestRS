//! A response shaper is read by `#[routes]`, and one it does not read is
//! refused in one sentence for all three — never expanded to the item in
//! silence. Two ways of leaving one unread: outside a `#[routes]` impl, where
//! any argument list used to compile, and under an import alias `#[routes]`
//! cannot recognise, where the route answered `200` with no header and no
//! `Location` while OpenAPI documented the same `200`.

use nest_rs::http::redirect as moved;
use nest_rs::http::{controller, routes};

struct Plain;

impl Plain {
    #[nest_rs::http::http_code("not a status at all")]
    #[nest_rs::http::response_header(42)]
    #[nest_rs::http::redirect(42, 42, 42)]
    fn helper(&self) {}
}

#[controller(path = "/moved")]
struct MovedController;

#[routes]
impl MovedController {
    #[get("/")]
    #[public]
    #[moved("https://example.com", 301)]
    async fn gone(&self) {}
}

fn main() {}
