//! `bind = Service` reads the subject's id from the route's one path
//! parameter; a path naming two leaves the id ambiguous, so the route is
//! refused at its path rather than answering `500` on its first request.

use nest_rs::http::{controller, routes};

struct Update;
struct PostsService;
struct Authorized<A, E>(A, E);
struct PostEntity;

#[controller(path = "/orgs")]
struct PostsController;

#[routes]
impl PostsController {
    #[post("/{org}/posts/{id}/publish")]
    #[authorize(Update, bind = PostsService)]
    async fn publish(&self, post: Authorized<Update, PostEntity>) -> String {
        let _ = post;
        String::new()
    }
}

fn main() {}
