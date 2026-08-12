//! A route has exactly one request body. `#[api(multipart = T)]` beside a
//! `Json<T>` extractor declares a second one, and whichever the document picked
//! would contradict what the handler actually reads — so the decorator refuses,
//! naming both halves.

use nest_rs_http::{controller, input, routes};

#[input]
struct CreatePost {
    title: String,
}

#[input]
struct UploadForm {
    file: String,
}

#[controller(path = "/posts")]
struct PostsController;

#[routes]
impl PostsController {
    #[post("/")]
    #[api(multipart = UploadForm)]
    async fn create(&self, body: poem::web::Json<CreatePost>) -> String {
        body.0.title
    }
}

fn main() {}
