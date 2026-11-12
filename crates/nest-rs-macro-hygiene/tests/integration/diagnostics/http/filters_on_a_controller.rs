//! `#[use_filters]` is refused beside a controller, naming
//! `#[use_exception_filters]`: an exception filter catching
//! `dyn Error + Send + Sync` is what replaced the untyped `Filter`.

use nest_rs::http::{controller, routes};

#[controller(path = "/items")]
#[use_filters(SomeFilter)]
struct ItemsController;

#[routes]
impl ItemsController {
    #[get("/")]
    async fn list(&self) -> &'static str {
        "items"
    }
}

fn main() {}
