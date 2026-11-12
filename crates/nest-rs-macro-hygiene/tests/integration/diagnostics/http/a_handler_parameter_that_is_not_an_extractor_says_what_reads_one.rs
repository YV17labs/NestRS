//! A handler parameter is read off the request through
//! `nest_rs::http::FromRequest`: a type that is no extractor is refused saying
//! it is no handler parameter and what reads a body, a path, a query and a
//! service, before rustc's own list of the trait's implementors.

use nest_rs::http::{controller, routes};

struct Settings;

#[controller(path = "/settings")]
struct SettingsController;

#[routes]
impl SettingsController {
    #[get("/")]
    async fn read(&self, _settings: Settings) -> &'static str {
        "ok"
    }
}

fn main() {}
