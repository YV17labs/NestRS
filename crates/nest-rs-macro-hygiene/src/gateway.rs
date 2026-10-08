//! `#[gateway]` + `#[use_guards]` + `#[messages]`: the guard-layer emission,
//! the message dispatch table and the versioned mount.

use nest_rs::core::{Layer, injectable};
use nest_rs::guards::{Denial, Guard, HttpGuard, async_trait};
use nest_rs::http::poem::Request as HttpRequest;
use nest_rs::ws::{gateway, messages};

/// Bound on the gateway struct, so it runs on the upgrade and owes [`HttpGuard`].
///
/// It overrides `check_http`: an empty `impl Guard` beside `impl HttpGuard` is
/// the lying attestation this crate's trybuild snapshots refuse.
#[injectable]
pub struct HygieneWsGuard;

impl Layer for HygieneWsGuard {}

#[async_trait]
impl Guard for HygieneWsGuard {
    async fn check_http(&self, _req: &mut HttpRequest) -> Result<(), Denial> {
        Ok(())
    }
}

impl HttpGuard for HygieneWsGuard {}

/// Guarded, so the `#[use_guards]` wrap is emitted.
#[gateway(path = "/hygiene")]
#[use_guards(HygieneWsGuard)]
pub struct HygieneGateway;

#[messages]
impl HygieneGateway {
    #[subscribe_message("hygiene.ping")]
    #[public]
    async fn ping(&self) {}

    #[subscribe_message("hygiene.sync")]
    #[public]
    fn sync(&self) -> String {
        "sync".into()
    }

    #[subscribe_message("hygiene.steady")]
    #[public]
    fn steady(&self) -> Result<String, crate::never::Never> {
        Ok("steady".into())
    }

    #[cfg(feature = "seaorm")]
    #[subscribe_message("hygiene.count")]
    #[authorize(nest_rs::authz::Read, crate::entity::Entity)]
    fn count(&self) -> Result<u64, std::fmt::Error> {
        Ok(0)
    }

    #[on_connect]
    fn connected(&self) {}

    /// A message compiled out takes its dispatch arm, chain and guard with it.
    #[cfg(any())]
    #[subscribe_message("hygiene.gone")]
    #[public]
    #[use_guards(crate::does_not_exist::Guard)]
    async fn compiled_out(
        &self,
        data: crate::does_not_exist::Data,
    ) -> crate::does_not_exist::Reply {
        crate::does_not_exist::answer(data)
    }

    /// A duplicate event and hook under an excluding condition are not a
    /// duplicate dispatch.
    #[cfg(any())]
    #[subscribe_message("hygiene.ping")]
    #[public]
    async fn ping_elsewhere(&self) {}

    #[cfg(any())]
    #[on_connect]
    fn connected_elsewhere(&self) {}
}

/// The versioned mount reaches `nest-rs-http`'s `version_path` through the umbrella.
#[gateway(path = "/hygiene", version = "1")]
pub struct HygieneVersionedGateway;

#[messages]
impl HygieneVersionedGateway {
    #[subscribe_message("hygiene.ping")]
    #[public]
    async fn ping(&self) {}
}
