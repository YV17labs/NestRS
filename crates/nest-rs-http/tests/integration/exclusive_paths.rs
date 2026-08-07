//! A mount path is its owner's exclusive namespace. Two controllers on one
//! prefix — or two self-mounted endpoints on one path — make poem panic deep in
//! route assembly (`duplicate path: <prefix>/*--poem-rest`). `configure` catches
//! both first and fails boot naming the two owners, so a wiring mistake reads
//! like every other nestrs boot error.

use nest_rs_core::{App, Container, ContainerBuilder, Transport, module};
use nest_rs_http::{HttpEndpointMeta, HttpTransport, controller, routes};
use poem::Route;

#[controller(path = "/users")]
struct UsersController;

#[routes]
impl UsersController {
    #[get("/")]
    async fn list(&self) -> &'static str {
        "users"
    }
}

/// A second controller deliberately claiming the same prefix.
#[controller(path = "/users")]
struct ShadowController;

#[routes]
impl ShadowController {
    #[get("/other")]
    async fn other(&self) -> &'static str {
        "shadow"
    }
}

#[module(providers = [UsersController, ShadowController])]
struct DuplicatePrefixModule;

/// Two self-mounted endpoints sharing one path — the shape `#[gateway]` emits,
/// and the shape a *cross-family* clash makes (an `#[mcp]` mount beside a
/// gateway on the same path). Two `#[mcp]` hosts on one path are **not** this
/// case: `nest-rs-mcp` aggregates them behind a single `HttpEndpointMeta`, so
/// they never reach this check. Attached by hand here because the rule belongs
/// to the transport, not to whichever surface happens to emit the meta.
struct FirstEndpoint;
struct SecondEndpoint;

impl nest_rs_core::Discoverable for FirstEndpoint {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<FirstEndpoint, HttpEndpointMeta>(
            HttpEndpointMeta::new("/tools", "mcp", |_c, r: Route| {
                r.at("/tools", poem::endpoint::make_sync(|_| "first"))
            })
            .owned_by("FirstTools")
            .exempt(),
        )
    }
}

impl nest_rs_core::Discoverable for SecondEndpoint {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<SecondEndpoint, HttpEndpointMeta>(
            HttpEndpointMeta::new("/tools", "mcp", |_c, r: Route| {
                r.at("/tools", poem::endpoint::make_sync(|_| "second"))
            })
            .owned_by("SecondTools")
            .exempt(),
        )
    }
}

#[module(providers = [FirstEndpoint, SecondEndpoint])]
struct DuplicateEndpointModule;

/// A self-mount claiming a path a **controller** already owns — the shape
/// `#[controller(path = "/chat")]` beside `#[gateway(path = "/chat")]` makes.
struct ChatSocket;

impl nest_rs_core::Discoverable for ChatSocket {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<ChatSocket, HttpEndpointMeta>(
            HttpEndpointMeta::new("/users", "ws", |_c, r: Route| {
                r.at("/users", poem::endpoint::make_sync(|_| "socket"))
            })
            .owned_by("ChatGateway"),
        )
    }
}

#[module(providers = [UsersController, ChatSocket])]
struct ControllerVersusEndpointModule;

async fn configure_error(container: &Container) -> String {
    let mut transport = HttpTransport::new();
    let err = transport
        .configure(container)
        .await
        .expect_err("a duplicated mount path must fail boot");
    err.to_string()
}

#[tokio::test]
async fn two_controllers_on_one_prefix_fail_boot_naming_both() {
    let app = App::builder()
        .module::<DuplicatePrefixModule>()
        .build()
        .await
        .expect("the module itself builds — the clash is a transport concern");

    let msg = configure_error(app.container()).await;
    assert!(
        msg.contains("duplicate controller prefix") && msg.contains("\"/users\""),
        "names the contested prefix: {msg}",
    );
    assert!(
        msg.contains("UsersController") && msg.contains("ShadowController"),
        "names both owners so the fix is obvious: {msg}",
    );
}

#[tokio::test]
async fn two_self_mounts_on_one_path_fail_boot_instead_of_panicking() {
    // The regression this pins: a second self-mount on one path used to reach
    // poem's route assembly and panic there, with no mention of either owner.
    let app = App::builder()
        .module::<DuplicateEndpointModule>()
        .build()
        .await
        .expect("the module itself builds");

    let msg = configure_error(app.container()).await;
    assert!(
        msg.contains("duplicate self-mounted endpoint path") && msg.contains("\"/tools\""),
        "names the contested path: {msg}",
    );
}

#[tokio::test]
async fn a_controller_and_a_self_mount_on_one_path_fail_boot_instead_of_panicking() {
    // The regression this pins: the exclusivity rule was enforced per family —
    // controllers against controllers, self-mounts against self-mounts — in two
    // maps that never met. A `#[controller(path = "/x")]` beside a
    // `#[gateway(path = "/x")]` passed both checks, logged both mounts as
    // successful, and then hit the very poem panic the check exists to prevent:
    // `panicked at poem/src/route/mod.rs: duplicate path: /x`.
    let app = App::builder()
        .module::<ControllerVersusEndpointModule>()
        .build()
        .await
        .expect("the module itself builds — the clash is a transport concern");

    let msg = configure_error(app.container()).await;
    assert!(
        msg.contains("duplicate mount path") && msg.contains("\"/users\""),
        "names the contested path: {msg}",
    );
    assert!(
        msg.contains("UsersController") && msg.contains("ChatGateway"),
        "names the controller AND the endpoint's owner so the fix is obvious: {msg}",
    );
}

#[tokio::test]
async fn a_self_mount_collision_names_the_owners_not_just_the_kind() {
    // `format!("a {} endpoint", label())` made the message a tautology —
    // "a ws endpoint and a ws endpoint both mount there" — useless with several
    // gateways in one app, while the controller twin named both owners.
    let app = App::builder()
        .module::<DuplicateEndpointModule>()
        .build()
        .await
        .expect("the module itself builds");

    let msg = configure_error(app.container()).await;
    assert!(
        msg.contains("FirstTools") && msg.contains("SecondTools"),
        "both owners must be named, not repeated as a kind: {msg}",
    );
}
