//! Boot-time phase validation of a gateway's **upgrade** chain.
//!
//! `#[routes]` has refused a misordered controller chain since the phase
//! declarations existed: an authorization guard listed before the
//! authentication guard whose principal it reads is a wiring mistake nothing
//! recovers from, so it fails the boot with a named error rather than answering
//! every request the same way forever.
//!
//! A gateway's upgrade is that same chain. `#[gateway(...)]`'s `#[use_guards]`
//! run on the HTTP `GET` that becomes the socket — `check_http`, the entry
//! `produced_principal` and `expected_principal` describe — so the check
//! transfers exactly, and it can run at boot because the mount is where a
//! gateway resolves its guards.
//!
//! What made the absence quiet is that it fails *closed*: the ability guard
//! finds no principal, installs nothing, and `Repo` denies every row. The socket
//! connects, messages are accepted, and every one of them comes back empty —
//! with nothing anywhere naming the order as the cause.
//!
//! **The per-message chains are not validated, and `guards-baseline.txt` records
//! why**: those run `check_ws_message`, where the principal contract this check
//! reads does not hold.

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

/// The path deliberately carries no word the assertions below look for. Spelled
/// `/ws/backwards` rather than `/ws/misordered-upgrade` because an earlier
/// version asserted `err.contains("upgrade")` against a path containing
/// "upgrade" — the assertion passed with the label removed entirely, which is a
/// green cell with nothing behind it.
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
    // The label, and the path *inside* it. An app with several gateways gets one
    // of these per misordered mount, and "a gateway" is not an address.
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
    /// Message-scope guards in the *reverse* order, deliberately. The
    /// per-message chain is not phase-validated, so this must still boot — and
    /// if a future change starts validating it, this test is what says so
    /// rather than a product failing to start.
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
    // The check is worth nothing if it refuses the ordering every app writes.
    TestApp::for_module::<OrderedModule>()
        .await
        .unwrap_or_else(|err| {
            panic!("authentication before authorization is the correct order: {err}")
        });
}

/// A gateway with no guards of its own, under a correctly ordered global pool —
/// the shape most apps have, and the one a check reading the wrong list would
/// refuse.
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
