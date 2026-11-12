//! A provider registered by hand states its residency itself: one that says
//! `SINGLETON = false` and nothing more is refused with the one cause it gave,
//! never the sentence of a scope it did not declare.

use nest_rs::core::{ProviderResidency, hooks};

struct Hand;

impl ProviderResidency for Hand {
    const SINGLETON: bool = false;
}

#[hooks]
impl Hand {
    #[on_module_init]
    async fn init(&self) {}
}

fn main() {}
