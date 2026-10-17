//! A handler parameter is read off the request through
//! `nest_rs::http::FromRequest`: a type that is no extractor is refused naming
//! the vocabulary's trait, never the router library's.

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
