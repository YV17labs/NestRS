//! A provider that *is* injectable and still cannot host these methods:
//! `scope = request` registers a factory, so `Container::get` outside a request
//! answers `None` and the work is skipped. The scope is known where
//! `#[injectable]` reads it, so the refusal belongs at compile time.

use nest_rs_core::injectable;
use nest_rs_events::listeners;

#[derive(Clone)]
struct Ping;

#[injectable(scope = request)]
#[derive(Default)]
struct PerRequest;

#[listeners]
impl PerRequest {
    #[on_event]
    async fn on_ping(&self, _event: Ping) {}
}

fn main() {}
