//! The worst of the shapes: `Container::get` on a `scope = transient`
//! provider **builds a throwaway**, so the methods run against an instance
//! nobody else holds and their effects are dropped — no skip, no warning, no
//! symptom.

use nest_rs_core::injectable;
use nest_rs_events::listeners;

#[derive(Clone)]
struct Ping;

#[injectable(scope = transient)]
#[derive(Default)]
struct PerResolution;

#[listeners]
impl PerResolution {
    #[on_event]
    async fn on_ping(&self, _event: Ping) {}
}

fn main() {}
