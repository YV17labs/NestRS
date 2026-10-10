//! A mount path is its owner's exclusive namespace: `configure` fails boot
//! naming both owners before poem would panic on the duplicate path.

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

/// Attached by hand: two `#[mcp]` hosts on one path aggregate behind one
/// `HttpEndpointMeta` and never reach this check.
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

// `Route::nest` appends a `/`, so `/x` and `/x/` are one key inside poem and
// panic as duplicates unless canonical before the boot check compares them.
#[test]
fn a_mount_path_is_canonical_before_anything_compares_it() {
    let mount = |path: &'static str| HttpEndpointMeta::new(path, "probe", |_c, r| r);

    assert_eq!(
        mount("/x/").path(),
        "/x",
        "a trailing slash is not a second mount"
    );
    assert_eq!(mount("x").path(), "/x", "…nor a missing leading one");
    assert_eq!(mount("  /x/  ").path(), "/x");
    assert_eq!(mount("/").path(), "/", "the root stays reachable");
    assert_eq!(mount("").path(), "/");
}

#[controller(path = "/parcels")]
struct ParcelsController;

#[routes]
impl ParcelsController {
    #[get("/{id}")]
    async fn read(&self) -> &'static str {
        "read"
    }
}

#[controller(path = "/")]
struct LockersController;

#[routes]
impl LockersController {
    #[delete("/parcels/{key}")]
    async fn release(&self) -> &'static str {
        "released"
    }
}

#[module(providers = [ParcelsController, LockersController])]
struct OneAddressTwoShapesModule;

#[tokio::test]
async fn two_shapes_of_one_address_fail_boot_naming_both_handlers() {
    let app = App::builder()
        .module::<OneAddressTwoShapesModule>()
        .build()
        .await
        .expect("the module itself builds — the clash is a transport concern");

    let msg = configure_error(app.container()).await;
    for named in [
        "ParcelsController::read",
        "\"/parcels/{id}\"",
        "LockersController::release",
        "\"/parcels/{key}\"",
    ] {
        assert!(msg.contains(named), "names {named}: {msg}");
    }
}

struct TemplatedSocket;

impl nest_rs_core::Discoverable for TemplatedSocket {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<TemplatedSocket, HttpEndpointMeta>(
            HttpEndpointMeta::new("/rooms/:room", "ws", |_c, r: Route| r).owned_by("RoomsGateway"),
        )
    }
}

#[module(providers = [TemplatedSocket])]
struct TemplatedSocketModule;

#[tokio::test]
async fn a_self_mount_path_out_of_the_grammar_fails_boot_instead_of_panicking() {
    let app = App::builder()
        .module::<TemplatedSocketModule>()
        .build()
        .await
        .expect("the module itself builds");

    let msg = configure_error(app.container()).await;
    assert!(
        msg.contains("RoomsGateway") && msg.contains("one literal address"),
        "names the owner and why: {msg}",
    );
}

struct DocsInThe6xSpelling;

impl nest_rs_core::Discoverable for DocsInThe6xSpelling {
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder.attach_meta::<DocsInThe6xSpelling, HttpEndpointMeta>(
            HttpEndpointMeta::new("/docs", "docs", |_c, r: Route| r)
                .also_mounts(["/docs-json/*rest"])
                .owned_by("DocsHost")
                .exempt(),
        )
    }
}

#[module(providers = [DocsInThe6xSpelling])]
struct DocsInThe6xSpellingModule;

#[tokio::test]
async fn an_also_mounts_entry_in_the_6x_spelling_fails_boot_naming_its_7_0_spelling() {
    let app = App::builder()
        .module::<DocsInThe6xSpellingModule>()
        .build()
        .await
        .expect("the module itself builds");

    let msg = configure_error(app.container()).await;
    assert!(
        msg.contains("docs endpoint DocsHost") && msg.contains("write `/docs-json/{*rest}`"),
        "names the owner and the 7.0 spelling: {msg}",
    );
}

#[tokio::test]
async fn an_imperative_mount_is_one_literal_address() {
    let app = App::builder().build().await.expect("an empty app builds");
    let mut transport =
        HttpTransport::new().mount("/files/{name}", |_| poem::endpoint::make_sync(|_| "file"));
    let msg = transport
        .configure(app.container())
        .await
        .expect_err("a templated imperative mount must fail boot")
        .to_string();
    assert!(
        msg.contains("HttpTransport::mount") && msg.contains("one literal address"),
        "names the mount and why: {msg}",
    );
}
