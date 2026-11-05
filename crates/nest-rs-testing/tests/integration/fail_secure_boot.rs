//! Boot-time fail-secure contract of the layer pool: a global layer injecting
//! what no module provides or that is not a singleton, a pool declared below
//! the module the app boots or twice, a controller guard no module provides, or
//! two controllers on one prefix, fail the boot.

use nest_rs_core::{
    Caught, ContainerBuilder, Discoverable, ExceptionFilter, GlobalExceptionFilters,
    GlobalInterceptors, Interceptor, Layer, MissingDependencyError, Module, Next, Outcome,
    Registering, ScopedLayerError, UnitContext, async_trait, injectable, module,
};
use nest_rs_guards::{Denial, GlobalGuards, Guard, GuardFor};
use nest_rs_http::{controller, routes};
use nest_rs_pipes::{GlobalPipe, GlobalPipes};
use nest_rs_testing::{HeadlessApp, TestApp};

/// Registered as a provider: the resolvable control case.
#[injectable]
#[derive(Default)]
struct WiredGuard;

impl Layer for WiredGuard {}

#[async_trait]
impl Guard for WiredGuard {
    fn phase(&self) -> nest_rs_guards::GuardPhase {
        nest_rs_guards::GuardPhase::Authorization
    }

    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for WiredGuard {}

/// Deliberately **not** listed in any module's providers.
#[injectable]
#[derive(Default)]
struct GhostGuard;

impl Layer for GhostGuard {}

#[async_trait]
impl Guard for GhostGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for GhostGuard {}

/// What no module provides, so what injects it cannot be built.
struct NobodyProvides;

/// A guard only its pool lists, injecting what no module provides.
#[expect(dead_code, reason = "the boot refuses it, so nothing reads it")]
#[injectable]
struct NeedyGuard {
    #[inject]
    nobody: std::sync::Arc<NobodyProvides>,
}

impl Layer for NeedyGuard {}

#[async_trait]
impl Guard for NeedyGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for NeedyGuard {}

/// An interceptor only its pool lists, injecting what no module provides.
#[expect(dead_code, reason = "the boot refuses it, so nothing reads it")]
#[injectable]
struct NeedyInterceptor {
    #[inject]
    nobody: std::sync::Arc<NobodyProvides>,
}

impl Layer for NeedyInterceptor {}

#[async_trait]
impl Interceptor for NeedyInterceptor {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

/// A filter only its pool lists, injecting what no module provides.
#[expect(dead_code, reason = "the boot refuses it, so nothing reads it")]
#[injectable]
struct NeedyExceptionFilter {
    #[inject]
    nobody: std::sync::Arc<NobodyProvides>,
}

impl Layer for NeedyExceptionFilter {}

#[async_trait]
impl ExceptionFilter for NeedyExceptionFilter {
    type Exception = std::fmt::Error;

    async fn catch(&self, _err: &std::fmt::Error, _cx: &mut UnitContext<'_>) -> Caught {
        Caught::Declined
    }
}

/// A pipe only its pool lists, injecting what no module provides.
#[expect(dead_code, reason = "the boot refuses it, so nothing reads it")]
#[injectable]
struct NeedyPipe {
    #[inject]
    nobody: std::sync::Arc<NobodyProvides>,
}

impl Layer for NeedyPipe {}

impl GlobalPipe for NeedyPipe {}

#[module(providers = [WiredGuard])]
struct GuardOnlyModule;

/// `TestApp` is not `Debug`, so unwrap the error arm by hand.
fn boot_error(result: anyhow::Result<TestApp>, expectation: &str) -> anyhow::Error {
    match result {
        Ok(_) => panic!("{expectation}"),
        Err(err) => err,
    }
}

/// The access graph's refusal of a global pool listing `layer`, which injects
/// what no module provides.
#[track_caller]
fn refused_unmet(result: anyhow::Result<HeadlessApp>, layer: &str) {
    let Err(err) = result else {
        panic!("a pool listing {layer}, which cannot be built, would boot without it");
    };
    let chain = format!("{err:#}");
    let unmet = err
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("the access graph refuses it: {chain}"));
    assert!(
        unmet.dependency.ends_with("NobodyProvides") && unmet.consumer.contains(layer),
        "the refusal names the declaration of {layer} and what it needs: {chain}"
    );
}

#[module(providers = [GlobalGuards<(NeedyGuard,)>])]
struct NeedyGuardPoolModule;

#[tokio::test]
async fn a_global_guard_that_cannot_be_built_fails_the_boot() {
    let err = boot_error(
        TestApp::for_module::<NeedyGuardPoolModule>().await,
        "a global guard that cannot be built must fail boot, not silently drop",
    );
    assert!(
        format!("{err:#}").contains("NeedyGuard"),
        "the error names the unresolvable guard: {err:#}",
    );
}

#[tokio::test]
async fn an_app_serving_no_http_still_fails_on_a_global_guard_that_cannot_be_built() {
    refused_unmet(
        HeadlessApp::builder()
            .module::<NeedyGuardPoolModule>()
            .build_headless()
            .await,
        "NeedyGuard",
    );
}

#[module(providers = [GlobalInterceptors<(NeedyInterceptor,)>])]
struct NeedyInterceptorPoolModule;

#[tokio::test]
async fn an_app_serving_no_http_still_fails_on_a_global_interceptor_that_cannot_be_built() {
    refused_unmet(
        HeadlessApp::builder()
            .module::<NeedyInterceptorPoolModule>()
            .build_headless()
            .await,
        "NeedyInterceptor",
    );
}

#[module(providers = [GlobalPipes<(NeedyPipe,)>])]
struct NeedyPipePoolModule;

#[tokio::test]
async fn an_app_serving_no_http_still_fails_on_a_global_pipe_that_cannot_be_built() {
    refused_unmet(
        HeadlessApp::builder()
            .module::<NeedyPipePoolModule>()
            .build_headless()
            .await,
        "NeedyPipe",
    );
}

#[module(providers = [GlobalExceptionFilters<(NeedyExceptionFilter,)>])]
struct NeedyExceptionFilterPoolModule;

#[tokio::test]
async fn an_app_serving_no_http_still_fails_on_a_global_filter_that_cannot_be_built() {
    refused_unmet(
        HeadlessApp::builder()
            .module::<NeedyExceptionFilterPoolModule>()
            .build_headless()
            .await,
        "NeedyExceptionFilter",
    );
}

/// A feature declaring the app's pool: an opening no line of the app wrote.
#[module(imports = [GuardOnlyModule], providers = [GlobalGuards<(WiredGuard,)>])]
struct FeatureDeclaringThePoolModule;

#[module(imports = [FeatureDeclaringThePoolModule])]
struct AppOverAFeaturesPoolModule;

#[tokio::test]
async fn a_global_pool_a_module_the_app_imports_declares_fails_the_boot_naming_it() {
    let (registry, chain) = crate::refused_wiring(
        HeadlessApp::builder()
            .module::<AppOverAFeaturesPoolModule>()
            .build_headless()
            .await,
        "a feature would choose the guards every route of the app runs",
    );
    assert_eq!(registry, "GlobalGuards", "{chain}");
    assert!(
        chain.contains("GlobalGuards<(WiredGuard,)>")
            && chain.contains("FeatureDeclaringThePoolModule")
            && chain.contains("the module the app boots"),
        "the refusal names the declaration, the module listing it and the remedy: {chain}",
    );
}

/// A binding installing the app's pool from a `register` written by hand: an
/// opening no module's providers show.
struct InstallsThePoolModule;

impl Module for InstallsThePoolModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        GlobalGuards::<(WiredGuard,)>::register(builder.import::<GuardOnlyModule>())
    }
}

#[module(imports = [InstallsThePoolModule])]
struct AppOverAHandWrittenPoolModule;

#[tokio::test]
async fn a_global_pool_no_modules_providers_list_fails_the_boot_naming_it() {
    let (registry, chain) = crate::refused_wiring(
        HeadlessApp::builder()
            .module::<AppOverAHandWrittenPoolModule>()
            .build_headless()
            .await,
        "a binding would choose the guards every route of the app runs",
    );
    assert_eq!(registry, "GlobalGuards", "{chain}");
    assert!(
        chain.contains("GlobalGuards<(WiredGuard,)>")
            && chain.contains("registered by no module's providers")
            && chain.contains("list it in `AppOverAHandWrittenPoolModule`'s providers"),
        "the refusal names the declaration, why, and the module to list it in: {chain}",
    );
}

#[module(imports = [GuardOnlyModule], providers = [GlobalGuards<(WiredGuard,)>])]
struct FirstPoolModule;

#[module(imports = [GuardOnlyModule], providers = [GlobalGuards<(WiredGuard, WiredGuard)>])]
struct SecondPoolModule;

#[tokio::test]
async fn a_global_pool_declared_twice_fails_the_boot_naming_both() {
    let (registry, chain) = crate::refused_wiring(
        HeadlessApp::builder()
            .module::<FirstPoolModule>()
            .module::<SecondPoolModule>()
            .build_headless()
            .await,
        "one of two pools would run without the other",
    );
    assert_eq!(registry, "GlobalGuards", "{chain}");
    assert!(
        chain.contains("declared twice")
            && chain.contains("`GlobalGuards<(WiredGuard,)>` in `FirstPoolModule`")
            && chain.contains("`GlobalGuards<(WiredGuard, WiredGuard)>` in `SecondPoolModule`"),
        "the refusal names both declarations and the module listing each: {chain}",
    );
}

#[module(imports = [GuardOnlyModule], providers = [GlobalGuards<(WiredGuard,)>])]
struct FirstPoolModuleTwin;

#[tokio::test]
async fn one_pool_two_roots_list_fails_the_boot_naming_both_roots() {
    let (registry, chain) = crate::refused_wiring(
        HeadlessApp::builder()
            .module::<FirstPoolModule>()
            .module::<FirstPoolModuleTwin>()
            .build_headless()
            .await,
        "one listing would hide the other",
    );
    assert_eq!(registry, "GlobalGuards", "{chain}");
    assert!(
        chain.contains("declared twice")
            && chain.contains("`GlobalGuards<(WiredGuard,)>` in `FirstPoolModule`")
            && chain.contains("`GlobalGuards<(WiredGuard,)>` in `FirstPoolModuleTwin`"),
        "the refusal tells the two listings apart by their modules: {chain}",
    );
}

#[module(
    imports = [FeatureDeclaringThePoolModule],
    providers = [GlobalGuards<(WiredGuard,)>]
)]
struct RootRelistingAFeaturesPoolModule;

#[tokio::test]
async fn one_pool_the_root_and_a_module_it_imports_list_fails_the_boot_naming_that_module() {
    let (registry, chain) = crate::refused_wiring(
        HeadlessApp::builder()
            .module::<RootRelistingAFeaturesPoolModule>()
            .build_headless()
            .await,
        "a feature would choose the guards every route of the app runs",
    );
    assert_eq!(registry, "GlobalGuards", "{chain}");
    assert!(
        chain
            .contains("`GlobalGuards<(WiredGuard,)>` is listed by `FeatureDeclaringThePoolModule`")
            && chain.contains("list it in `RootRelistingAFeaturesPoolModule`'s providers"),
        "the refusal names the imported module listing the pool: {chain}",
    );
}

/// Rebuilt on every resolution: a pool, resolved once at mount, would share
/// one instance with every request.
#[injectable(scope = transient)]
#[derive(Default)]
struct TransientGuard;

impl Layer for TransientGuard {}

#[async_trait]
impl Guard for TransientGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }

    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for TransientGuard {}

#[module(providers = [TransientGuard, GlobalGuards<(TransientGuard,)>])]
struct TransientGuardPoolModule;

/// Built per request: a pool, resolved once at mount, would never build it.
#[injectable(scope = request)]
#[derive(Default)]
struct PerRequestInterceptor;

impl Layer for PerRequestInterceptor {}

#[async_trait]
impl Interceptor for PerRequestInterceptor {
    async fn intercept(&self, cx: &mut UnitContext<'_>, next: Next<'_>) -> Outcome {
        next.run(cx).await
    }
}

#[module(providers = [PerRequestInterceptor, GlobalInterceptors<(PerRequestInterceptor,)>])]
struct PerRequestInterceptorPoolModule;

/// The access graph's refusal of `pool` listing `layer`, which is not a
/// singleton.
#[track_caller]
fn refused_scoped(result: anyhow::Result<HeadlessApp>, pool: &str, layer: &str) {
    let Err(err) = result else {
        panic!("{pool} would run {layer} as one instance shared by every request");
    };
    let chain = format!("{err:#}");
    let scoped = err
        .downcast_ref::<ScopedLayerError>()
        .unwrap_or_else(|| panic!("refused as a layer, not as an injection: {chain}"));
    assert!(
        scoped.host == pool && scoped.layer.ends_with(layer),
        "the refusal names the pool and the layer: {chain}",
    );
    assert!(
        chain.contains(&format!("Make `{}` a singleton", scoped.layer)),
        "…and the one remedy a pool's layer has: {chain}",
    );
}

#[tokio::test]
async fn a_transient_layer_in_a_global_pool_fails_the_boot_naming_it_and_the_pool() {
    refused_scoped(
        HeadlessApp::builder()
            .module::<TransientGuardPoolModule>()
            .build_headless()
            .await,
        "GlobalGuards<(TransientGuard,)>",
        "TransientGuard",
    );
}

#[tokio::test]
async fn a_request_scoped_layer_in_a_global_pool_fails_the_boot_naming_it_and_the_pool() {
    refused_scoped(
        HeadlessApp::builder()
            .module::<PerRequestInterceptorPoolModule>()
            .build_headless()
            .await,
        "GlobalInterceptors<(PerRequestInterceptor,)>",
        "PerRequestInterceptor",
    );
}

/// Gated by a guard no module provides.
#[controller(path = "/ghost-gated")]
#[use_guards(GhostGuard)]
struct GhostGatedController;

#[routes]
impl GhostGatedController {
    #[get("/")]
    async fn read(&self) -> &'static str {
        "unreachable"
    }
}

#[module(providers = [GhostGatedController])]
struct GhostGatedModule;

#[tokio::test]
async fn a_controller_guard_no_module_provides_fails_the_boot_naming_it_and_the_controller() {
    // Built by hand, so the access graph does not refuse first.
    let container = nest_rs_core::Container::builder()
        .provide(nest_rs_http::HttpConfig::default())
        .import::<GhostGatedModule>()
        .build()
        .expect("the container builds");
    let mut transport =
        nest_rs_http::HttpTransport::from_config(&nest_rs_http::HttpConfig::default())
            .expect("the defaults are in range");
    let Err(err) = nest_rs_core::Transport::configure(&mut transport, &container).await else {
        panic!("the controller would serve without the guard it declares");
    };
    let chain = format!("{err:#}");
    assert!(
        chain.contains("GhostGuard") && chain.contains("GhostGatedController"),
        "the refusal names the guard and the controller declaring it: {chain}",
    );
    assert!(
        chain.contains("no imported module provides it"),
        "…and the remedy: {chain}",
    );
}

/// Controller routes with no guard and no `#[public]` marker: an opening no
/// line declares.
#[controller(path = "/")]
struct OpenController;

#[routes]
impl OpenController {
    #[post("/thing")]
    async fn create(&self) -> String {
        "made".into()
    }

    #[get("/thing")]
    async fn read(&self) -> String {
        "read".into()
    }

    #[get("/open")]
    #[public]
    async fn open(&self) -> String {
        "open on purpose".into()
    }
}

#[module(providers = [OpenController])]
struct OpenModule;

#[tokio::test]
async fn a_route_with_no_guard_and_no_public_refuses_the_boot_naming_every_such_route() {
    let err = boot_error(
        TestApp::for_module::<OpenModule>().await,
        "a route no guard covers and no #[public] opens would serve every caller",
    );
    let chain = format!("{err:#}");
    assert!(
        chain.contains("POST /thing (create)") && chain.contains("GET /thing (read)"),
        "one refusal names every such route, not the first one: {chain}",
    );
    assert!(
        !chain.contains("/open"),
        "a route marked #[public] is open on purpose: {chain}",
    );
    assert!(
        chain.contains(nest_rs_http::__private::NO_GUARD_REMEDY),
        "the one remedy of a surface no guard runs at: {chain}",
    );
    for named in [
        "`GlobalGuards<(...)>`",
        "`#[use_guards(...)]` on a controller or a gateway",
        "`EndpointGuards<View, (...)>` on an endpoint carrying units",
        "`#[public]`",
        "`McpModule::for_root(McpAccess::Public)`",
    ] {
        assert!(chain.contains(named), "the remedy names {named}: {chain}");
    }
}

#[module(imports = [OpenModule, GuardOnlyModule], providers = [GlobalGuards<(WiredGuard,)>])]
struct GuardedOpenModule;

#[tokio::test]
async fn the_global_guard_pool_covers_a_route_declaring_no_posture() {
    let app = TestApp::for_module::<GuardedOpenModule>()
        .await
        .expect("a route the app's pool guards declares its posture there");
    app.http().post("/thing").send().await.assert_status_is_ok();
}

/// Admits every caller and decides nothing about who it is, as a rate limit does:
/// it declares no phase.
#[injectable]
#[derive(Default)]
struct LimitGuard;

impl Layer for LimitGuard {}

#[async_trait]
impl Guard for LimitGuard {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for LimitGuard {}

#[controller(path = "/limited")]
#[use_guards(LimitGuard)]
struct LimitedByItsControllerController;

#[routes]
impl LimitedByItsControllerController {
    #[get("/controller")]
    async fn by_its_controller(&self) -> String {
        "reached by any caller".into()
    }

    #[get("/public")]
    #[public]
    async fn opened(&self) -> String {
        "the opening it says".into()
    }
}

#[controller(path = "/limited/method")]
struct LimitedByItsMethodController;

#[routes]
impl LimitedByItsMethodController {
    #[get("/")]
    #[use_guards(LimitGuard)]
    async fn by_its_method(&self) -> String {
        "reached by any caller".into()
    }
}

#[module(providers = [LimitGuard, LimitedByItsControllerController, LimitedByItsMethodController])]
struct LimitedRoutesModule;

/// A rate limit, on the route or its controller, opens the route with no line.
#[tokio::test]
async fn a_route_under_guards_that_identify_no_one_refuses_the_boot_naming_each() {
    let err = boot_error(
        TestApp::for_module::<LimitedRoutesModule>().await,
        "a guard identifying no one covers no route",
    );
    let chain = format!("{err:#}");
    assert!(
        chain.contains("GET /limited/controller (by_its_controller)")
            && chain.contains("GET /limited/method (by_its_method)"),
        "{chain}",
    );
    assert!(!chain.contains("/limited/public"), "{chain}");
    assert!(
        chain.contains("phase()") && chain.contains("GuardPhase::Authentication"),
        "a guard of one's own declaring no phase is told what to declare: {chain}",
    );
}

#[module(imports = [OpenModule, LimitOnlyModule], providers = [GlobalGuards<(LimitGuard,)>])]
struct LimitedByThePoolModule;

#[module(providers = [LimitGuard])]
struct LimitOnlyModule;

#[tokio::test]
async fn a_pool_of_guards_that_identify_no_one_covers_no_route() {
    let err = boot_error(
        TestApp::for_module::<LimitedByThePoolModule>().await,
        "a pool identifying no one covers no route",
    );
    let chain = format!("{err:#}");
    assert!(
        chain.contains("POST /thing (create)") && chain.contains("GET /thing (read)"),
        "{chain}",
    );
    assert!(!chain.contains("/open"), "{chain}");
}

#[controller(path = "/limited-open")]
#[use_guards(LimitGuard)]
struct OpenUnderALimitController;

#[routes]
impl OpenUnderALimitController {
    #[get("/")]
    #[public]
    async fn opened(&self) -> String {
        "the opening it says".into()
    }
}

#[module(providers = [LimitGuard, OpenUnderALimitController])]
struct OpenUnderALimitModule;

#[tokio::test]
async fn public_under_a_guard_that_identifies_no_one_boots_and_answers_a_visitor() {
    let app = TestApp::for_module::<OpenUnderALimitModule>()
        .await
        .expect("`#[public]` is the visible opening a rate limit stands beside");
    app.http()
        .get("/limited-open")
        .send()
        .await
        .assert_text("the opening it says")
        .await;
}

struct GatedFiles;

static GATED_FILES: nest_rs_core::Handler = nest_rs_http::declare_endpoint::<GatedFiles>(
    "GatedFiles",
    "/gated-files",
    nest_rs_core::Posture::Gated,
);

impl Discoverable for GatedFiles {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<Self, nest_rs_http::HttpEndpointMeta>(
            nest_rs_http::HttpEndpointMeta::new(
                &GATED_FILES,
                "/gated-files",
                "files",
                |_container, _routes| {},
            )
            .owned_by("GatedFiles"),
        )
    }
}

#[module(providers = [LimitGuard, GatedFiles])]
struct GatedFilesModule;

#[module(imports = [GatedFilesModule], providers = [GlobalGuards<(LimitGuard,)>])]
struct LimitedGatedFilesApp;

/// A gated surface carrying no unit: the same opening with nothing written.
#[tokio::test]
async fn a_gated_self_mount_under_guards_that_identify_no_one_refuses_the_boot() {
    let err = boot_error(
        TestApp::for_module::<LimitedGatedFilesApp>().await,
        "a rate limit opens a gated surface with no line",
    );
    let chain = format!("{err:#}");
    assert!(
        chain.contains("/gated-files (files)") && chain.contains("phase()"),
        "{chain}",
    );
}

/// Two controllers claiming the same prefix: a boot refusal naming both, never
/// a panic deep in route assembly.
#[controller(path = "/dup")]
struct FirstDupController;

#[routes]
impl FirstDupController {
    #[post("/")]
    #[public]
    async fn first(&self) -> String {
        "first".into()
    }
}

#[controller(path = "/dup")]
struct SecondDupController;

#[routes]
impl SecondDupController {
    #[post("/")]
    #[public]
    async fn second(&self) -> String {
        "second".into()
    }
}

#[module(providers = [FirstDupController, SecondDupController])]
struct DuplicatePrefixModule;

#[tokio::test]
async fn two_controllers_sharing_a_prefix_fail_boot_naming_both() {
    let result = TestApp::builder()
        .module::<DuplicatePrefixModule>()
        .build()
        .await;
    let err = boot_error(
        result,
        "two controllers on one prefix must fail boot, not panic inside the router",
    );
    let msg = err.to_string();
    assert!(
        msg.contains("/dup")
            && msg.contains("FirstDupController")
            && msg.contains("SecondDupController"),
        "the error names the shared prefix and both controllers: {err}",
    );
}
