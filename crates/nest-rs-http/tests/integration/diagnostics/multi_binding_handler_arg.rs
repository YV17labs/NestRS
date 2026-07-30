//! A destructured handler argument forwards under the identifier it binds, so
//! `Path(name): Path<String>` works — poem's own idiom. A pattern binding
//! **two** names has no single identifier to forward, and this pins the error
//! that says so.
//!
//! It replaces the snapshot that used to pin a blanket "arguments must be simple
//! identifiers": the general restriction is gone, and only this residue of it
//! remains. Pinned because the message has to name the way out, not just the
//! refusal — a developer hitting it copied the two-element form from poem, where
//! it is legal.

use nest_rs_http::{controller, routes};
use poem::web::Path;

#[controller(path = "/pairs")]
struct PairController;

#[routes]
impl PairController {
    #[get("/:a/:b")]
    #[public]
    async fn pair(&self, Path((a, b)): Path<(String, String)>) -> String {
        format!("{a}-{b}")
    }
}

fn main() {}
