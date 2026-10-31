//! The third HTTP emitter, and the one that carried no snapshot.
//!
//! A `#[use_guards]` on the `#[gateway]` struct runs on the **upgrade**, which
//! is an HTTP `GET` — so it owes `GuardFor<HttpView>`, not `GuardFor<WsView>`,
//! and the error has to point at `#[gateway]` rather than at `#[messages]`: the
//! guard is written on the struct, and that is the line the author is looking
//! at.
//!
//! Its sibling `http_only_guard_on_a_message` is the mirror image — a guard that
//! *does* check HTTP, refused per message where `check` is what runs. Between
//! them the two scopes cannot be confused for one another.

use nest_rs::core::{Layer, UnitContext, async_trait, injectable};
use nest_rs::guards::{Denial, Guard};
use nest_rs::ws::{gateway, messages};

#[injectable]
#[derive(Default)]
struct UnattestedGuard;

impl Layer for UnattestedGuard {}

// Its decision is written, its edge never declared: the declaration is what
// says which units this decision was written for.
#[async_trait]
impl Guard for UnattestedGuard {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

#[gateway(path = "/ws")]
#[use_guards(UnattestedGuard)]
struct DemoGateway;

#[messages]
impl DemoGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) {}
}

fn main() {}
