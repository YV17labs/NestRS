//! Two verbs at one address are one mount, and each parameter binds by the name
//! the address was mounted under — so the address is spelled one way.

use nest_rs::http::{controller, routes};

#[controller(path = "/parcels")]
struct ParcelsController;

#[routes]
impl ParcelsController {
    #[get("/{id}")]
    async fn read(&self) -> String {
        "read".into()
    }

    #[delete("/{other}")]
    async fn remove(&self) -> String {
        "removed".into()
    }
}

fn main() {}
