//! `api(…)` documents the routes `#[crud]` generates on a controller; a
//! resolver serves no OpenAPI operation, so the key is refused there by name.

use nest_rs::graphql::{crud, resolver};

#[resolver]
struct OrgsResolver;

#[crud(service = OrgsService, api(get(summary = "Get an org by id")))]
impl OrgsResolver {}

fn main() {}
