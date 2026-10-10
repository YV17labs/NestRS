//! Boot-time phase validation of a gateway's **upgrade** chain. The per-message
//! chains are not validated: they run `check_ws_message`, where the principal
//! contract this check reads does not hold.

use nest_rs_core::{Layer, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardPhase, HttpGuard, PrincipalClaim, WsGuard};
use nest_rs_testing::TestApp;
use nest_rs_ws::nest_rs_http::poem::Request as HttpRequest;
use nest_rs_ws::{WsClient, WsModule, async_trait, gateway, messages};

/// What the authentication guard attaches and the authorization guard reads.
#[derive(Clone)]
struct Caller;

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

    async fn check_http(&self, req: &mut HttpRequest) -> Result<(), Denial> {
        req.extensions_mut().insert(Caller);
        Ok(())
    }
}

impl HttpGuard for Authenticate {}
impl WsGuard for Authenticate {}

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

impl HttpGuard for Authorize {}
impl WsGuard for Authorize {}

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

impl HttpGuard for Unprovided {}
impl WsGuard for Unprovided {}

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
        .import::<UnprovidedModule>()
        .build();
    let mut transport = nest_rs_ws::nest_rs_http::HttpTransport::default();
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
        .import::<UnprovidedUpgradeModule>()
        .build();
    let mut transport = nest_rs_ws::nest_rs_http::HttpTransport::default();
    let Err(err) = nest_rs_core::Transport::configure(&mut transport, &container).await else {
        panic!("the upgrade would run without the guard it declares");
    };
    let chain = format!("{err:#}");
    assert!(
        chain.contains("Unprovided") && chain.contains("/ws/unprovided-upgrade"),
        "the refusal names the guard and the gateway declaring it: {chain}",
    );
}
