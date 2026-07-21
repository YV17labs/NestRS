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

/// Two self-mounted endpoints (the shape `#[mcp]` / `#[gateway]` emit) sharing
/// one path.
struct FirstEndpoint;
struct SecondEndpoint;

impl nest_rs_core::Discoverable for FirstEndpoint {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<FirstEndpoint, HttpEndpointMeta>(
            HttpEndpointMeta::new("/tools", "mcp", |_c, r: Route| {
                r.at("/tools", poem::endpoint::make_sync(|_| "first"))
            })
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
            .exempt(),
        )
    }
}

#[module(providers = [FirstEndpoint, SecondEndpoint])]
struct DuplicateEndpointModule;

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
    // The regression this pins: a second `#[mcp(path = "/mcp")]` used to reach
    // poem's route assembly and panic there, with no mention of either host.
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
