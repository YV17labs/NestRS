//! Every host-scope layer on the impl half names the struct it belongs on, not
//! only `#[use_guards]`: `#[use_interceptors]` there reached rustc as `cannot
//! find attribute`.

use nest_rs::http::{controller, routes};

struct Timing;

#[controller(path = "/widgets")]
struct WidgetsController;

#[routes]
#[use_interceptors(Timing)]
impl WidgetsController {
    #[get("/")]
    #[public]
    async fn list(&self) -> String {
        String::new()
    }
}

fn main() {}
