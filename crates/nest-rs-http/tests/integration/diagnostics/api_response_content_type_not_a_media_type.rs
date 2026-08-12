//! `response_content_type` keys the `content` map of the operation's success
//! response, so a value that is not a media type produces a document no client
//! can match a response against. The decorator has the literal in hand and
//! rejects it there, rather than leaving it to be discovered in the document.

use nest_rs_http::{controller, routes};

#[controller(path = "/exports")]
struct ExportsController;

#[routes]
impl ExportsController {
    #[get("/")]
    #[api(response_content_type = "octet-stream")]
    async fn export(&self) -> String {
        "bytes".into()
    }
}

fn main() {}
