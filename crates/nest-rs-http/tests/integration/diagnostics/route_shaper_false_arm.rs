//! HTTP-D1: `#[routes]` arms the response shaper by *type*, so a local struct
//! that borrows the name `Authorize`/`Bind` without implementing
//! `RouteResponseShaper` would simply select nothing and leave the route
//! unshaped. The eager assertion turns that silence into a spanned error on
//! the parameter itself, naming the trait.

use nest_rs_http::{controller, routes};
use poem::web::Json;
use poem::Result;

struct Authorize<T>(T);

#[controller(path = "/orgs")]
struct OrgsController;

#[routes]
impl OrgsController {
    #[get("/")]
    async fn list(&self, _authz: Authorize<u8>) -> Result<Json<u8>> {
        Ok(Json(0))
    }
}

fn main() {}
