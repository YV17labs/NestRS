//! Boot-time validation of a gateway's chains: its **upgrade**'s phases and
//! principals, each message's read after them, and its posture.

use nest_rs_core::{AmbientValue, Layer, async_trait, injectable, module};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor, GuardPhase, PrincipalClaim};
use nest_rs_testing::TestApp;
use nest_rs_ws::{WsClient, WsConfig, WsModule, gateway, messages};

/// What the authentication guard attaches and the authorization guard reads.
#[derive(Clone)]
struct Caller;

impl AmbientValue for Caller {}

#[injectable]
#[derive(Default, Layer)]
struct Authenticate;

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

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for Authenticate {}
impl GuardFor<nest_rs_ws::WsView> for Authenticate {}

#[injectable]
#[derive(Default, Layer)]
struct Authorize;

#[async_trait]
impl Guard for Authorize {
    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }

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
        Err(err) => format!("{err:#}"),
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
    /// Reverse order on purpose: each first ran at the upgrade, in order, so listing
    /// them again here replays what ran and misorders nothing.
    #[subscribe_message("ping")]
    #[use_guards(Authorize, Authenticate)]
    async fn ping(&self, _client: &WsClient) -> String {
        "pong".into()
    }
}

#[module(imports = [WsModule], providers = [OrderedGateway, Authenticate, Authorize])]
struct OrderedModule;

#[tokio::test]
async fn a_message_listing_again_what_its_upgrade_ran_is_no_misorder() {
    TestApp::for_module::<OrderedModule>()
        .await
        .unwrap_or_else(|err| {
            panic!("authentication before authorization is the correct order: {err}")
        });
}

/// Establishes who the caller is with nothing a guard above reads, at a message alone.
#[injectable]
#[derive(Default, Layer)]
struct AuthenticateAtTheMessage;

#[async_trait]
impl Guard for AuthenticateAtTheMessage {
    fn phase(&self) -> GuardPhase {
        GuardPhase::Authentication
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_ws::WsView> for AuthenticateAtTheMessage {}

/// Decides what the caller may do, needing no principal to boot.
#[injectable]
#[derive(Default, Layer)]
struct AuthorizeAnyone;

#[async_trait]
impl Guard for AuthorizeAnyone {
    fn phase(&self) -> GuardPhase {
        GuardPhase::Authorization
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AuthorizeAnyone {}
impl GuardFor<nest_rs_ws::WsView> for AuthorizeAnyone {}

#[gateway(path = "/ws/misordered-message")]
#[use_guards(Authenticate)]
struct MisorderedMessageGateway;

#[messages]
impl MisorderedMessageGateway {
    #[subscribe_message("ping")]
    #[use_guards(AuthorizeAnyone, AuthenticateAtTheMessage)]
    async fn ping(&self, _client: &WsClient) -> String {
        "pong".into()
    }
}

#[module(
    imports = [WsModule],
    providers = [
        MisorderedMessageGateway,
        Authenticate,
        AuthorizeAnyone,
        AuthenticateAtTheMessage,
    ],
)]
struct MisorderedMessageModule;

#[gateway(path = "/ws/authenticating-after-its-upgrade")]
#[use_guards(AuthorizeAnyone)]
struct AfterItsUpgradeGateway;

#[messages]
impl AfterItsUpgradeGateway {
    #[subscribe_message("ping")]
    #[use_guards(AuthenticateAtTheMessage)]
    async fn ping(&self, _client: &WsClient) -> String {
        "pong".into()
    }
}

#[module(
    imports = [WsModule],
    providers = [AfterItsUpgradeGateway, AuthorizeAnyone, AuthenticateAtTheMessage],
)]
struct AfterItsUpgradeModule;

/// `boot` refused for an authentication guard after an authorization guard, naming the two.
fn assert_misordered(boot: nest_rs_core::anyhow::Result<TestApp>) {
    let Err(refused) = boot else {
        panic!("the misordered chain booted");
    };
    let refused = format!("{refused:#}");
    assert!(
        refused.contains("AuthenticateAtTheMessage") && refused.contains("AuthorizeAnyone"),
        "{refused}",
    );
    assert!(refused.contains("after authorization guard"), "{refused}");
}

#[tokio::test]
async fn a_message_listing_authentication_after_authorization_refuses_the_boot() {
    assert_misordered(TestApp::for_module::<MisorderedMessageModule>().await);
}

#[tokio::test]
async fn a_message_authenticating_after_its_upgrades_authorization_refuses_the_boot() {
    assert_misordered(TestApp::for_module::<AfterItsUpgradeModule>().await);
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
#[derive(Default, Layer)]
struct Unprovided;

#[async_trait]
impl Guard for Unprovided {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

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

/// A mount has no `Result`: the message's site files its refusal on the
/// container, and the boot fails on it once its transports are configured.
#[tokio::test]
async fn a_message_guard_no_module_provides_fails_the_boot_naming_it_and_its_message() {
    // Built by hand, so neither the access graph nor the wiring step refuses first.
    let container = nest_rs_core::Container::builder()
        .provide(WsConfig::default())
        .provide(nest_rs_http::HttpConfig::default())
        .import::<UnprovidedModule>()
        .build()
        .expect("the container builds");
    let mut transport =
        nest_rs_http::HttpTransport::from_config(&nest_rs_http::HttpConfig::default())
            .expect("the defaults are in range");
    nest_rs_core::Transport::configure(&mut transport, &container)
        .await
        .expect("the mount files its refusal rather than returning it");
    let Some(err) = nest_rs_core::__private::take_site_refusal(&container) else {
        panic!("the message would run without the guard it declares");
    };
    let chain = format!("{err:#}");
    assert!(
        chain.contains("Unprovided") && chain.contains("UnprovidedGateway::ping"),
        "the refusal names the guard and the message declaring it: {chain}",
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
        .provide(nest_rs_http::HttpConfig::default())
        .import::<UnprovidedUpgradeModule>()
        .build()
        .expect("the container builds");
    let mut transport =
        nest_rs_http::HttpTransport::from_config(&nest_rs_http::HttpConfig::default())
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
#[derive(Default, Layer)]
struct CountingGuard;

#[async_trait]
impl Guard for CountingGuard {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

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
    #[force_guards(CountingGuard)]
    async fn again(&self) -> &'static str {
        "ok"
    }
}

#[module(imports = [WsModule], providers = [ReplayedGateway, CountingGuard])]
struct ReplayedModule;

#[module(imports = [ReplayedModule], providers = [GlobalGuards<(CountingGuard,)>])]
struct AGuardForcedAloneOnAMessageRunsAgainAfterTheGlobalPoolApp;

#[tokio::test]
async fn a_guard_forced_alone_on_a_message_runs_again_after_the_global_pool() {
    let app = TestApp::builder()
        .module::<AGuardForcedAloneOnAMessageRunsAgainAfterTheGlobalPoolApp>()
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

/// A pooled guard deciding on the HTTP request: in `check_http`, its `check`
/// admitting, it gates the upgrade and every message the socket carries
/// answers; in `check`, it runs on each message too, which has no HTTP view,
/// and the line says which guard and what to move.
mod request_decided {
    use nest_rs_core::{Layer, UnitContext, async_trait, injectable, module};
    use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
    use nest_rs_http::{HttpView, StatusCode};
    use nest_rs_testing::{LogCapture, TestApp};
    use nest_rs_ws::{WsModule, gateway, messages};

    fn keyed(cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        match HttpView::of(cx)?.request().header("x-api-key") {
            Some(_) => Ok(()),
            None => Err(Denial::unauthorized("missing key")),
        }
    }

    #[injectable]
    #[derive(Default, Layer)]
    struct KeyAtTheRequest;

    #[async_trait]
    impl Guard for KeyAtTheRequest {
        fn phase(&self) -> nest_rs_guards::GuardPhase {
            nest_rs_guards::GuardPhase::Authorization
        }

        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            keyed(cx)
        }

        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl GuardFor<HttpView> for KeyAtTheRequest {}
    impl GuardFor<nest_rs_ws::WsView> for KeyAtTheRequest {}

    #[injectable]
    #[derive(Default, Layer)]
    struct KeyAtTheUnit;

    #[async_trait]
    impl Guard for KeyAtTheUnit {
        fn phase(&self) -> nest_rs_guards::GuardPhase {
            nest_rs_guards::GuardPhase::Authorization
        }

        async fn check_http(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            keyed(cx)
        }

        async fn check(&self, cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            keyed(cx)
        }
    }

    impl GuardFor<HttpView> for KeyAtTheUnit {}
    impl GuardFor<nest_rs_ws::WsView> for KeyAtTheUnit {}

    #[gateway(path = "/ws/keyed")]
    struct KeyedGateway;

    #[messages]
    impl KeyedGateway {
        #[subscribe_message("ping")]
        async fn ping(&self) -> &'static str {
            "pong"
        }
    }

    #[module(
        imports = [WsModule],
        providers = [KeyAtTheRequest, KeyAtTheUnit, KeyedGateway],
    )]
    struct KeyModule;

    #[module(imports = [KeyModule], providers = [GlobalGuards<(KeyAtTheRequest,)>])]
    struct RequestKeyPoolModule;

    #[module(imports = [KeyModule], providers = [GlobalGuards<(KeyAtTheUnit,)>])]
    struct UnitKeyPoolModule;

    #[tokio::test]
    async fn a_pooled_guard_deciding_in_check_http_gates_the_upgrade_and_admits_its_messages() {
        let app = TestApp::builder()
            .module::<RequestKeyPoolModule>()
            .build_ws()
            .await
            .expect("boots");

        let mut socket = app
            .socket("/ws/keyed")
            .header("x-api-key", "k")
            .connect()
            .await;
        socket.send("ping", serde_json::Value::Null).await;
        assert_eq!(socket.next_envelope().await["data"], "pong");
        assert_eq!(
            app.socket("/ws/keyed").expect_refused().await,
            StatusCode::UNAUTHORIZED
        );
        app.shutdown().await.expect("shuts down");
    }

    #[tokio::test]
    async fn a_pooled_guard_reading_the_request_in_check_refuses_each_message_naming_itself() {
        let logs = LogCapture::install();
        let app = TestApp::builder()
            .module::<UnitKeyPoolModule>()
            .build_ws()
            .await
            .expect("boots");

        let mut socket = app
            .socket("/ws/keyed")
            .header("x-api-key", "k")
            .connect()
            .await;
        for _ in 0..2 {
            socket.send("ping", serde_json::Value::Null).await;
            let envelope = socket.next_envelope().await;
            assert_eq!(envelope["data"]["code"], "INTERNAL", "{envelope}");
        }

        let denials = logs.find("nest_rs::layers", "guard denied the message");
        assert_eq!(denials.len(), 2, "each refused message is filed");
        for denial in denials {
            assert_eq!(denial.level, "error");
            assert_eq!(denial.field("guard").as_deref(), Some("KeyAtTheUnit"));
            assert_eq!(denial.field("unit").as_deref(), Some("ws.message"));
            assert!(
                denial
                    .field("error")
                    .is_some_and(|chain| chain.contains("decide the request in `check_http`")),
                "the edge's line says the edit: {denial:?}"
            );
        }
        logs.expect_none(
            "nest_rs::http",
            "a guard read the HTTP request of a unit with no HTTP view",
        );
        app.shutdown().await.expect("shuts down");
    }
}

/// What the boot says of a gateway's posture: a message no guard covers, and a
/// `#[public]` message no visitor could reach.
mod posture {
    use nest_rs_core::{Layer, Module, UnitContext, async_trait, injectable, module};
    use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor, GuardPhase};
    use nest_rs_http::HttpView;
    use nest_rs_testing::TestApp;
    use nest_rs_testing::ws::WsApp;
    use nest_rs_ws::{WsClient, WsModule, WsView, gateway, messages};

    use super::Authenticate;

    /// Admits every request and decides nothing about who is calling, as a rate
    /// limit does.
    #[injectable]
    #[derive(Default, Layer)]
    struct Throttle;

    #[async_trait]
    impl Guard for Throttle {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
    }

    impl GuardFor<HttpView> for Throttle {}
    impl GuardFor<WsView> for Throttle {}

    async fn refusal<M: Module + 'static>() -> String {
        match TestApp::for_module::<M>().await {
            Ok(_) => panic!("the app must not boot"),
            Err(err) => format!("{err:#}"),
        }
    }

    async fn served<M: Module + 'static>() -> WsApp {
        TestApp::builder()
            .module::<M>()
            .build_ws()
            .await
            .expect("the gateway boots")
    }

    async fn answer(app: &WsApp, path: &str, event: &str) -> serde_json::Value {
        let mut socket = app.socket(path).connect().await;
        socket.send(event, serde_json::json!({})).await;
        socket.next_envelope().await["data"].take()
    }

    #[gateway(path = "/ws/open/uncovered")]
    struct UncoveredGateway;

    #[messages]
    impl UncoveredGateway {
        #[subscribe_message("first")]
        async fn first(&self, _client: &WsClient) -> String {
            "first".into()
        }

        #[subscribe_message("second")]
        async fn second(&self, _client: &WsClient) -> String {
            "second".into()
        }

        #[subscribe_message("said")]
        #[public]
        async fn said(&self, _client: &WsClient) -> String {
            "said".into()
        }
    }

    #[module(imports = [WsModule], providers = [UncoveredGateway])]
    struct UncoveredModule;

    #[tokio::test]
    async fn a_message_no_guard_covers_refuses_the_boot_naming_every_such_message() {
        let said = refusal::<UncoveredModule>().await;
        assert!(
            said.contains("/ws/open/uncovered")
                && said.contains("first")
                && said.contains("second"),
            "{said}",
        );
        assert!(!said.contains("said"), "that one opened itself: {said}");
        assert!(
            said.contains(nest_rs_http::__private::NO_GUARD_REMEDY),
            "the one remedy of a surface no guard runs at: {said}",
        );
    }

    #[gateway(path = "/ws/open/hooks")]
    struct HooksOnlyGateway;

    #[messages]
    impl HooksOnlyGateway {
        #[on_connect]
        async fn joined(&self, client: &WsClient) {
            client.join("lobby");
        }
    }

    #[module(imports = [WsModule], providers = [HooksOnlyGateway])]
    struct HooksOnlyModule;

    #[tokio::test]
    async fn a_gateway_declaring_hooks_alone_refuses_the_boot() {
        let said = refusal::<HooksOnlyModule>().await;
        assert!(
            said.contains("/ws/open/hooks")
                && said.contains(nest_rs_http::__private::NO_GUARD_REMEDY),
            "its hooks run on any connection, and no line says so: {said}",
        );
    }

    #[gateway(path = "/ws/open/guarded-messages")]
    struct GuardedMessagesGateway;

    #[messages]
    impl GuardedMessagesGateway {
        #[on_connect]
        async fn joined(&self, client: &WsClient) {
            client.join("lobby");
        }

        #[subscribe_message("guarded")]
        #[use_guards(Policy)]
        async fn guarded(&self, _client: &WsClient) -> String {
            "guarded".into()
        }
    }

    #[module(imports = [WsModule], providers = [Policy, GuardedMessagesGateway])]
    struct GuardedMessagesModule;

    /// A message's guard runs on that message alone: any caller still connects,
    /// its hooks run, and what the server emits reaches the socket.
    #[tokio::test]
    async fn an_open_upgrade_under_messages_guarding_themselves_refuses_the_boot() {
        let said = refusal::<GuardedMessagesModule>().await;
        assert!(said.contains("/ws/open/guarded-messages"), "{said}");
        assert!(
            said.contains(nest_rs_http::__private::NO_GUARD_REMEDY),
            "the one remedy of a surface no guard runs at: {said}",
        );
    }

    #[gateway(path = "/ws/covered/gateway")]
    #[use_guards(Authenticate)]
    struct CoveredGateway;

    #[messages]
    impl CoveredGateway {
        #[subscribe_message("ping")]
        async fn ping(&self, _client: &WsClient) -> String {
            "covered by the gateway".into()
        }
    }

    #[module(imports = [WsModule], providers = [Authenticate, CoveredGateway])]
    struct CoveredModule;

    #[tokio::test]
    async fn a_message_under_the_gateways_guards_needs_no_posture() {
        let app = served::<CoveredModule>().await;
        assert_eq!(
            answer(&app, "/ws/covered/gateway", "ping").await,
            "covered by the gateway",
        );
    }

    #[gateway(path = "/ws/covered/pool")]
    struct PooledGateway;

    #[messages]
    impl PooledGateway {
        #[subscribe_message("ping")]
        async fn ping(&self, _client: &WsClient) -> String {
            "covered by the pool".into()
        }
    }

    #[module(imports = [WsModule], providers = [Authenticate, PooledGateway])]
    struct PooledModule;

    #[module(imports = [PooledModule], providers = [GlobalGuards<(Authenticate,)>])]
    struct PooledApp;

    #[tokio::test]
    async fn a_message_under_the_pool_needs_no_posture() {
        let app = served::<PooledApp>().await;
        assert_eq!(
            answer(&app, "/ws/covered/pool", "ping").await,
            "covered by the pool",
        );
    }

    #[gateway(path = "/ws/covered/beside")]
    struct BesideGateway;

    #[messages]
    impl BesideGateway {
        #[subscribe_message("ping")]
        #[use_guards(Policy)]
        async fn ping(&self, _client: &WsClient) -> String {
            "covered beside it".into()
        }

        #[subscribe_message("open")]
        #[public]
        async fn open(&self, _client: &WsClient) -> String {
            "the line opening the connection".into()
        }
    }

    #[module(imports = [WsModule], providers = [Policy, BesideGateway])]
    struct BesideModule;

    #[tokio::test]
    async fn a_guard_beside_the_message_covers_it_too() {
        let app = served::<BesideModule>().await;
        assert_eq!(
            answer(&app, "/ws/covered/beside", "ping").await,
            "covered beside it",
        );
    }

    #[gateway(path = "/ws/limited/gateway")]
    #[use_guards(Throttle)]
    struct LimitedGateway;

    #[messages]
    impl LimitedGateway {
        #[on_connect]
        async fn joined(&self, client: &WsClient) {
            client.join("lobby");
        }

        #[subscribe_message("silent")]
        async fn silent(&self, _client: &WsClient) -> String {
            "reached by any caller, with no line saying so".into()
        }

        #[subscribe_message("also_silent")]
        #[use_guards(Throttle)]
        async fn also_silent(&self, _client: &WsClient) -> String {
            "and neither here".into()
        }

        #[subscribe_message("said")]
        #[public]
        async fn said(&self, _client: &WsClient) -> String {
            "the opening it says".into()
        }
    }

    #[module(imports = [WsModule], providers = [Throttle, LimitedGateway])]
    struct LimitedModule;

    /// A rate limit identifies no one, so a message under it alone is an opening
    /// nothing says.
    #[tokio::test]
    async fn a_message_under_a_guard_that_identifies_no_one_refuses_the_boot_naming_each() {
        let said = refusal::<LimitedModule>().await;
        assert!(
            said.contains("/ws/limited/gateway")
                && said.contains("silent")
                && said.contains("also_silent"),
            "{said}",
        );
        assert!(!said.contains(": silent, also_silent, said"), "{said}");
        assert!(
            said.contains(nest_rs_http::__private::NO_GUARD_REMEDY),
            "the one remedy, which names a guard's phase: {said}",
        );
        assert!(
            said.contains("phase()") && said.contains("GuardPhase::Authentication"),
            "{said}",
        );
    }

    #[gateway(path = "/ws/limited/pool")]
    struct LimitedByThePoolGateway;

    #[messages]
    impl LimitedByThePoolGateway {
        #[subscribe_message("silent")]
        async fn silent(&self, _client: &WsClient) -> String {
            "reached by any caller, with no line saying so".into()
        }
    }

    #[module(imports = [WsModule], providers = [Throttle, LimitedByThePoolGateway])]
    struct LimitedByThePoolModule;

    #[module(imports = [LimitedByThePoolModule], providers = [GlobalGuards<(Throttle,)>])]
    struct LimitedByThePoolApp;

    #[tokio::test]
    async fn the_pool_of_a_guard_that_identifies_no_one_covers_no_message() {
        let said = refusal::<LimitedByThePoolApp>().await;
        assert!(
            said.contains("/ws/limited/pool") && said.contains("silent"),
            "{said}"
        );
    }

    #[gateway(path = "/ws/limited/opened")]
    #[use_guards(Throttle)]
    struct OpenedUnderALimitGateway;

    #[messages]
    impl OpenedUnderALimitGateway {
        #[subscribe_message("said")]
        #[public]
        async fn said(&self, _client: &WsClient) -> String {
            "the opening it says".into()
        }
    }

    #[module(imports = [WsModule], providers = [Throttle, OpenedUnderALimitGateway])]
    struct OpenedUnderALimitModule;

    #[tokio::test]
    async fn public_under_a_guard_that_identifies_no_one_boots_and_answers_a_visitor() {
        let app = served::<OpenedUnderALimitModule>().await;
        assert_eq!(
            answer(&app, "/ws/limited/opened", "said").await,
            "the opening it says",
        );
    }

    #[gateway(path = "/ws/mislabelled/authenticated")]
    #[use_guards(Authenticate)]
    struct MislabelledGateway;

    #[messages]
    impl MislabelledGateway {
        #[subscribe_message("mislabelled")]
        #[public]
        async fn mislabelled(&self, _client: &WsClient) -> String {
            "reached by authenticated callers alone".into()
        }

        #[subscribe_message("honest")]
        async fn honest(&self, _client: &WsClient) -> String {
            "covered".into()
        }
    }

    #[module(imports = [WsModule], providers = [Authenticate, MislabelledGateway])]
    struct MislabelledModule;

    #[tokio::test]
    async fn public_behind_an_authenticating_upgrade_refuses_the_boot_it_can_open_nothing() {
        let said = refusal::<MislabelledModule>().await;
        assert!(
            said.contains("mislabelled") && said.contains("Authenticate"),
            "the refusal names the message and the guard its upgrade runs: {said}",
        );
        assert!(!said.contains("honest"), "{said}");
        assert!(
            said.contains("drop `#[public]`"),
            "a covered message needs no posture: {said}",
        );
        assert!(
            said.contains("take that guard off the upgrade"),
            "a guard the gateway binds, it can unbind: {said}",
        );
    }

    /// Reads who is calling, a visitor's branch of its own.
    #[injectable]
    #[derive(Default, Layer)]
    struct Policy;

    #[async_trait]
    impl Guard for Policy {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }

        fn phase(&self) -> GuardPhase {
            GuardPhase::Authorization
        }
    }

    impl GuardFor<HttpView> for Policy {}
    impl GuardFor<WsView> for Policy {}

    #[gateway(path = "/ws/mislabelled/authorized")]
    #[use_guards(Policy)]
    struct MislabelledByAuthorizationGateway;

    #[messages]
    impl MislabelledByAuthorizationGateway {
        #[subscribe_message("mislabelled")]
        #[public]
        async fn mislabelled(&self, _client: &WsClient) -> String {
            "reached by authorized callers alone".into()
        }
    }

    #[module(imports = [WsModule], providers = [Policy, MislabelledByAuthorizationGateway])]
    struct MislabelledByAuthorizationModule;

    #[tokio::test]
    async fn public_behind_an_authorizing_upgrade_refuses_the_boot_too() {
        let said = refusal::<MislabelledByAuthorizationModule>().await;
        assert!(
            said.contains("mislabelled") && said.contains("Policy"),
            "{said}"
        );
    }

    #[gateway(path = "/ws/mislabelled/pool")]
    struct MislabelledByThePoolGateway;

    #[messages]
    impl MislabelledByThePoolGateway {
        #[subscribe_message("mislabelled")]
        #[public]
        async fn mislabelled(&self, _client: &WsClient) -> String {
            "reached by authenticated callers alone".into()
        }
    }

    #[module(imports = [WsModule], providers = [Authenticate, MislabelledByThePoolGateway])]
    struct MislabelledByThePoolModule;

    #[module(imports = [MislabelledByThePoolModule], providers = [GlobalGuards<(Authenticate,)>])]
    struct MislabelledByThePoolApp;

    #[tokio::test]
    async fn the_pools_authentication_refuses_a_public_message_as_the_gateways_does() {
        let said = refusal::<MislabelledByThePoolApp>().await;
        assert!(
            said.contains("Authenticate") && said.contains("#[public]"),
            "{said}"
        );
        assert!(
            !said.contains("take that guard off") && said.contains("from the pool"),
            "no gateway takes a pool's guard off its own upgrade, so opening one means \
             moving the guard: {said}",
        );
    }

    #[gateway(path = "/ws/open/public")]
    struct PublicGateway;

    #[messages]
    impl PublicGateway {
        #[subscribe_message("ping")]
        #[public]
        async fn ping(&self, _client: &WsClient) -> String {
            "a visitor reads this".into()
        }
    }

    #[module(imports = [WsModule], providers = [PublicGateway])]
    struct PublicModule;

    #[tokio::test]
    async fn public_on_an_open_gateway_admits_a_visitor() {
        let app = served::<PublicModule>().await;
        assert_eq!(
            answer(&app, "/ws/open/public", "ping").await,
            "a visitor reads this",
        );
    }

    #[gateway(path = "/ws/open/throttled")]
    #[use_guards(Throttle)]
    struct ThrottledPublicGateway;

    #[messages]
    impl ThrottledPublicGateway {
        #[subscribe_message("ping")]
        #[public]
        async fn ping(&self, _client: &WsClient) -> String {
            "a visitor reads this, throttled".into()
        }
    }

    #[module(imports = [WsModule], providers = [Throttle, ThrottledPublicGateway])]
    struct ThrottledPublicModule;

    #[tokio::test]
    async fn public_beside_a_guard_that_identifies_no_one_stays_an_opening() {
        let app = served::<ThrottledPublicModule>().await;
        assert_eq!(
            answer(&app, "/ws/open/throttled", "ping").await,
            "a visitor reads this, throttled",
        );
    }
}
