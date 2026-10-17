//! Boot-time phase validation of a gateway's **upgrade** chain. The per-message
//! chains are not validated: they run `check`, where the principal contract
//! this check reads does not hold.

use nest_rs_core::{AmbientValue, Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsClient, WsConfig, WsModule, async_trait, gateway, messages};

/// What the authentication guard attaches and the authorization guard reads.
#[derive(Clone)]
struct Caller;

impl AmbientValue for Caller {}

#[injectable]
#[derive(Default)]
struct Authenticate;

impl Layer for Authenticate {}

#[async_trait]
impl Guard for Authenticate {
    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }

    fn produced_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<Caller>())
    }

    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        cx.attach(Caller);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for Authenticate {}
impl GuardFor<nest_rs_ws::WsView> for Authenticate {}

#[injectable]
#[derive(Default)]
struct Authorize;

impl Layer for Authorize {}

#[async_trait]
impl Guard for Authorize {
    fn phase(&self) -> GuardPhase {
        GuardPhase::Authorization
    }

    fn expected_principal(&self) -> Option<PrincipalClaim> {
        Some(PrincipalClaim::of::<Caller>())
    }
}

impl GuardFor<nest_rs_http::HttpView> for Authorize {}
impl GuardFor<nest_rs_ws::WsView> for Authorize {}

/// The path carries no word the assertions below look for, so they cannot pass
/// on the path alone.
#[gateway(path = "/ws/backwards")]
#[use_guards(Authorize, Authenticate)]
struct BackwardsGateway;

#[messages]
impl BackwardsGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self, _client: &WsClient) -> String {
        "pong".into()
    }
}

#[module(imports = [WsModule], providers = [BackwardsGateway, Authorize, Authenticate])]
struct BackwardsModule;

#[tokio::test]
async fn a_misordered_upgrade_chain_fails_the_boot_naming_both_guards_and_the_gateway() {
    let err = match TestApp::for_module::<BackwardsModule>().await {
        Ok(_) => panic!("a gateway whose upgrade chain is misordered must not boot"),
        Err(err) => err.to_string(),
    };

    assert!(
        err.contains("Authenticate") && err.contains("Authorize"),
        "the failure names both guards — the order is a property of the pair, \
         and one name sends the reader to a guard that is fine: {err}",
    );
    assert!(
        err.contains("gateway upgrade") && err.contains("/ws/backwards"),
        "…and says which mount: {err}",
    );
}

#[gateway(path = "/ws/ordered")]
#[use_guards(Authenticate, Authorize)]
struct OrderedGateway;

#[messages]
impl OrderedGateway {
    /// Reverse order on purpose: the per-message chain is not phase-validated.
    #[subscribe_message("ping")]
    #[use_guards(Authorize, Authenticate)]
    #[public]
    async fn ping(&self, _client: &WsClient) -> String {
        "pong".into()
    }
}

#[module(imports = [WsModule], providers = [OrderedGateway, Authenticate, Authorize])]
struct OrderedModule;

#[tokio::test]
async fn a_correct_upgrade_chain_boots_whatever_the_messages_declare() {
    TestApp::for_module::<OrderedModule>()
        .await
        .unwrap_or_else(|err| {
            panic!("authentication before authorization is the correct order: {err}")
        });
}

#[gateway(path = "/ws/bare")]
struct BareGateway;

#[messages]
impl BareGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self, _client: &WsClient) -> String {
        "pong".into()
    }
}

#[module(imports = [WsModule], providers = [BareGateway])]
struct BareModule;

#[tokio::test]
async fn a_gateway_that_declares_no_guards_boots() {
    TestApp::for_module::<BareModule>()
        .await
        .unwrap_or_else(|err| panic!("a gateway need not declare guards: {err}"));
}

/// Listed by a message's `#[use_guards]`, provided by no module.
#[injectable]
#[derive(Default)]
struct Unprovided;

impl Layer for Unprovided {}

#[async_trait]
impl Guard for Unprovided {}

impl GuardFor<nest_rs_http::HttpView> for Unprovided {}
impl GuardFor<nest_rs_ws::WsView> for Unprovided {}

#[gateway(path = "/ws/unprovided")]
struct UnprovidedGateway;

#[messages]
impl UnprovidedGateway {
    #[subscribe_message("ping")]
    #[use_guards(Unprovided)]
    #[public]
    async fn ping(&self, _client: &WsClient) -> String {
        "pong".into()
    }
}

#[module(imports = [WsModule], providers = [UnprovidedGateway])]
struct UnprovidedModule;

#[tokio::test]
async fn a_message_guard_no_module_provides_fails_the_boot_naming_it_and_its_event() {
    // Built by hand, so the access graph does not refuse first.
    let container = nest_rs_core::Container::builder()
        .provide(WsConfig::default())
        .import::<UnprovidedModule>()
        .build();
    let mut transport = nest_rs_ws::nest_rs_http::HttpTransport::from_config(
        &nest_rs_ws::nest_rs_http::HttpConfig::default(),
    )
    .expect("the defaults are in range");
    let Err(err) = nest_rs_core::Transport::configure(&mut transport, &container).await else {
        panic!("the event would run without the guard it declares");
    };
    let chain = format!("{err:#}");
    assert!(
        chain.contains("Unprovided") && chain.contains("ws ping"),
        "the refusal names the guard and the event declaring it: {chain}",
    );
    assert!(
        chain.contains("no imported module provides it"),
        "…and the remedy: {chain}",
    );
}

/// Gated by a guard no module provides.
#[gateway(path = "/ws/unprovided-upgrade")]
#[use_guards(Unprovided)]
struct UnprovidedUpgradeGateway;

#[messages]
impl UnprovidedUpgradeGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self, _client: &WsClient) -> String {
        "pong".into()
    }
}

#[module(imports = [WsModule], providers = [UnprovidedUpgradeGateway])]
struct UnprovidedUpgradeModule;

#[tokio::test]
async fn an_upgrade_guard_no_module_provides_fails_the_boot_naming_it_and_the_gateway() {
    let container = nest_rs_core::Container::builder()
        .provide(WsConfig::default())
        .import::<UnprovidedUpgradeModule>()
        .build();
    let mut transport = nest_rs_ws::nest_rs_http::HttpTransport::from_config(
        &nest_rs_ws::nest_rs_http::HttpConfig::default(),
    )
    .expect("the defaults are in range");
    let Err(err) = nest_rs_core::Transport::configure(&mut transport, &container).await else {
        panic!("the upgrade would run without the guard it declares");
    };
    let chain = format!("{err:#}");
    assert!(
        chain.contains("Unprovided") && chain.contains("/ws/unprovided-upgrade"),
        "the refusal names the guard and the gateway declaring it: {chain}",
    );
}

static FORCED_CHECKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Counts every message check, so a replay is seen.
#[injectable]
#[derive(Default)]
struct CountingGuard;

impl Layer for CountingGuard {}

#[async_trait]
impl Guard for CountingGuard {
    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        FORCED_CHECKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for CountingGuard {}
impl GuardFor<nest_rs_ws::WsView> for CountingGuard {}

#[gateway(path = "/ws/replayed")]
struct ReplayedGateway;

#[messages]
impl ReplayedGateway {
    #[subscribe_message("again")]
    #[public]
    #[force_guards(CountingGuard)]
    async fn again(&self) -> &'static str {
        "ok"
    }
}

#[module(imports = [WsModule], providers = [ReplayedGateway, CountingGuard])]
struct ReplayedModule;

#[tokio::test]
async fn a_guard_forced_alone_on_a_message_runs_again_after_the_global_pool() {
    let app = TestApp::builder()
        .module::<ReplayedModule>()
        .use_guards_global([nest_rs_guards::guard::<CountingGuard>()])
        .build_ws()
        .await
        .expect("a gateway boots on a real port");

    let mut socket = app.socket("/ws/replayed").connect().await;
    socket.send("again", serde_json::Value::Null).await;
    let reply = socket.next_envelope().await;
    assert_eq!(reply["data"], "ok", "{reply}");
    assert_eq!(
        FORCED_CHECKS.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the pool's check, then the replay `#[force_guards]` binds on the message",
    );
    app.shutdown().await.expect("the transport stops cleanly");
}
