//! HTTP-D1: `#[routes]` arms the response shaper by scanning parameter type
//! *names* for `Authorize`/`Bind`. A local type that borrows one of those names
//! without implementing `RouteResponseShaper` would otherwise fail as a
//! confusing transitive `Endpoint` bound at the mount site — the eager probe
//! turns it into a spanned error on the parameter itself.

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
