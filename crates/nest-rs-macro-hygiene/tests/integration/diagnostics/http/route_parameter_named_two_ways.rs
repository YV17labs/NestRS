//! Two verbs at one address mount one poem node, and poem binds a parameter by
//! the name the node was mounted under — so the address is spelled one way.

use nest_rs::http::{controller, routes};

#[controller(path = "/parcels")]
struct ParcelsController;

#[routes]
impl ParcelsController {
    #[get("/:id")]
    async fn read(&self) -> String {
        "read".into()
    }

    #[delete("/:other")]
    async fn remove(&self) -> String {
        "removed".into()
    }
}

fn main() {}
