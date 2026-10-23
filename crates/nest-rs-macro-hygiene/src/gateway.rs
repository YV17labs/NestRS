//! `#[gateway]` + `#[use_guards]` + `#[messages]`: the guard-layer emission,
//! a guard a message binds or `#[force_guards]` re-runs, the interceptors,
//! exception filters and data pipes a gateway and its messages bind, a
//! `Scoped<T>` parameter, the message sites and the versioned mount.

use nest_rs::core::{Layer, UnitContext, injectable};
use nest_rs::guards::{Denial, Guard, GuardFor, async_trait};
use nest_rs::http::HttpView;
use nest_rs::pipes::GlobalPipe;
use nest_rs::ws::{Scoped, WsView, gateway, messages};

/// Bound on the gateway struct, so it runs on the upgrade and owes
/// `GuardFor<HttpView>`.
///
/// It overrides `check_http`: an empty `impl Guard` beside the declaration is
/// the lying one this crate's trybuild snapshots refuse.
#[injectable]
pub struct HygieneWsGuard;

impl Layer for HygieneWsGuard {}

#[async_trait]
impl Guard for HygieneWsGuard {
    async fn check_http(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<HttpView> for HygieneWsGuard {}

/// Bound on one message, and forced on another past the app-wide pool.
#[injectable]
pub struct HygieneMessageGuard;

impl Layer for HygieneMessageGuard {}

#[async_trait]
impl Guard for HygieneMessageGuard {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<WsView> for HygieneMessageGuard {}

/// A value `#[meta]` declares, read by type.
pub struct HygieneLimit(pub u32);

/// Reads a message's `data` and changes nothing: the pipe a gateway and its
/// messages bind.
#[injectable]
#[derive(Default)]
pub struct HygieneDataPipe;

impl Layer for HygieneDataPipe {}

impl GlobalPipe for HygieneDataPipe {}

/// Built for each message, read through a `Scoped<T>` parameter.
#[injectable(scope = request)]
pub struct HygieneMessageSession;

/// Gated, so the `#[use_guards]` wrap is emitted; its interceptor, exception
/// filter and pipe run around, and over, each of its messages.
#[gateway(path = "/hygiene")]
#[meta(HygieneLimit(100))]
#[use_guards(HygieneWsGuard)]
#[use_interceptors(crate::pipeline::HygieneUnitAudit)]
#[use_exception_filters(crate::pipeline::HygieneUnitRefusal)]
#[use_pipes(HygieneDataPipe)]
pub struct HygieneGateway;

#[messages]
impl HygieneGateway {
    #[subscribe_message("hygiene.ping")]
    #[meta(HygieneLimit(10))]
    #[public]
    async fn ping(&self) {}

    #[subscribe_message("hygiene.again")]
    #[public]
    #[force_guards(HygieneMessageGuard)]
    async fn again(&self) {}

    #[subscribe_message("hygiene.checked")]
    #[public]
    #[use_guards(HygieneMessageGuard)]
    async fn checked(&self) {}

    #[subscribe_message("hygiene.layered")]
    #[public]
    #[use_interceptors(crate::pipeline::HygieneUnitAudit)]
    #[use_exception_filters(crate::pipeline::HygieneUnitRefusal)]
    #[use_pipes(HygieneDataPipe)]
    async fn layered(&self, session: Scoped<HygieneMessageSession>, text: String) -> String {
        let _ = session;
        text
    }

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
    #[meta(HygieneLimit(1))]
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
