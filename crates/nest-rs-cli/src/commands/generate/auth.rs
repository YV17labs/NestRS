//! `nestrs g auth` — the app-side authn/authz adapter every guarded feature
//! imports: the `Claims` principal, `AppAuthnGuard`/`AppAuthnModule`, and
//! `AppAbility`/`AppAuthzGuard`/`AppAuthzHttpModule`.
//!
//! The framework is generic over the principal and the policy, so these types
//! cannot ship in a `nest-rs-*` crate — they are app code, identical in every
//! project until you edit the rules. Generating them is what makes
//! `#[use_guards(AppAuthnGuard, AppAuthzGuard)]` resolve; without it a guarded
//! controller names two types nothing defines.

use std::path::PathBuf;

use super::cargo::{auth_deps, ensure_features_deps, ensure_workspace_deps};
use super::support::{finish, resolve_start, wire_into_app};
use crate::context::{Context, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::Transport;
use crate::scaffold::{Scaffold, ensure_lines};
use crate::templates::auth;

pub struct AuthOptions {
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

pub fn run(opts: AuthOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.clone().ok_or(CliError::NotNestrsWorkspace)?;

    if exists(&ws) {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "`{}` already has an auth adapter — edit `app_authz/ability.rs` to change the policy",
            ws.root.display()
        )));
    }

    let mut s = Scaffold::new();
    queue(&mut s, &ws, Vec::new());
    s.edit(
        ws.root.join("Cargo.toml"),
        ensure_workspace_deps(auth_deps()),
    );
    s.edit(ws.features_cargo(), ensure_features_deps(auth_deps()));
    s.edit(ws.features_lib(), ensure_lines(lib_decls()));
    let wired_app = wire(&ctx, &mut s);

    finish(s, opts.dry_run, &ws.root, "the auth adapter")?;
    let env_prefix = crate::context::env_prefix();
    print_next_steps(&env_prefix, wired_app.is_some());
    Ok(())
}

/// The `features` crate root declarations the adapter needs. Returned rather
/// than queued so a caller adding its own can fold them into **one**
/// `ensure_lines` — two `edit`s on the same file each re-read it from disk, and
/// the second write would clobber the first.
pub(super) fn lib_decls() -> Vec<String> {
    [
        "pub mod app_authn;",
        "pub mod app_authz;",
        "pub use app_authn::{Claims, Role};",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Queue every auth file and the `.env` secret — but none of the shared-file
/// edits ([`lib_decls`], [`auth_deps`]), which a caller adding its own must
/// fold into a single `edit` per path. Split out so `g resource` can bootstrap
/// the adapter in the same transaction as the resource that needs it.
///
/// `authz_decls` are extra index lines for `app_authz/mod.rs` — a caller
/// scaffolding a transport bridge in the same transaction passes
/// [`AuthzBridge::decls`], since the file is created here and an `edit` targets
/// what is already on disk.
pub(super) fn queue(s: &mut Scaffold, ws: &NestrsWorkspace, authz_decls: Vec<String>) {
    let src = ws.features_root();

    // Every verbatim file, in one table — the same shape as
    // [`AuthzBridge::queue`]. The two files below the loop are the ones a
    // table cannot say: `app_authz/mod.rs` folds in the caller's index lines, and
    // `.env` is an edit whenever the file already exists.
    const FILES: [(&str, &str); 11] = [
        ("app_authn/claims.rs", auth::AUTHN_CLAIMS),
        ("app_authn/mod.rs", auth::AUTHN_MOD),
        ("app_authn/module.rs", auth::AUTHN_MODULE),
        ("app_authn/strategy.rs", auth::AUTHN_STRATEGY),
        ("app_authn/http/mod.rs", auth::AUTHN_HTTP_MOD),
        ("app_authn/http/audit.rs", auth::AUTHN_HTTP_AUDIT),
        ("app_authn/http/guard.rs", auth::AUTHN_HTTP_GUARD),
        ("app_authn/http/controller.rs", auth::AUTHN_HTTP_CONTROLLER),
        ("app_authn/http/module.rs", auth::AUTHN_HTTP_MODULE),
        ("app_authz/ability.rs", auth::AUTHZ_ABILITY),
        ("app_authz/module.rs", auth::AUTHZ_MODULE),
    ];
    for (path, body) in FILES {
        s.create(src.join(path), body.to_string());
    }

    let authz_mod =
        ensure_lines(authz_decls)(auth::AUTHZ_MOD).unwrap_or_else(|| auth::AUTHZ_MOD.to_string());
    s.create(src.join("app_authz/mod.rs"), authz_mod);
    HTTP_BRIDGE.queue(s, ws);

    // Every scaffolded workspace has one; a hand-rolled tree may not, and a
    // missing `.env` is not a reason to refuse the whole adapter.
    // Rendered, not copied: the key has to carry this project's prefix, or the
    // scaffolded secret is a line the app never reads and auth refuses to boot.
    // One placeholder, so a `Renderer` (and the `Names` it needs) would be two
    // dozen substitutions for nothing.
    let env_prefix = crate::context::env_prefix();
    let env_authn = auth::ENV_AUTHN.replace("{{env_prefix}}", &env_prefix);
    let env = ws.root.join(".env");
    if env.is_file() {
        s.edit(env, append_authn_secret(&env_prefix, env_authn));
    } else {
        s.create(env, env_authn.trim_start().to_string());
    }
}

/// Both roots are listed at the composition site even though `AppAuthzHttpModule`
/// pulls `AppAuthnModule` in transitively — an app's `module.rs` is the inventory
/// of the concerns it serves. Returned rather than wired here so a caller
/// bootstrapping the adapter folds them into its own single `module.rs` edit.
pub(super) const APP_IMPORTS: [(&str, &str); 3] = [
    ("features::app_authn::AppAuthnModule", "AppAuthnModule"),
    (
        "features::app_authn::AppAuthnHttpModule",
        "AppAuthnHttpModule",
    ),
    (
        "features::app_authz::AppAuthzHttpModule",
        "AppAuthzHttpModule",
    ),
];

fn wire(ctx: &Context, s: &mut Scaffold) -> Option<PathBuf> {
    wire_into_app(ctx, s, &APP_IMPORTS, None)
}

pub(super) fn exists(ws: &NestrsWorkspace) -> bool {
    ws.features_root().join("app_authz").is_dir()
}

// ── authz/<transport>/ — the bridge a guarded adapter is enforced through ───
//
// Every transport bridge lives here, HTTP's included: `authz/` is one tree with
// one layout, and which generator happens to want a bridge first is not a reason
// to scatter them. One table rather than a trio of near-identical helpers per
// transport — three copies is how `ws` and `mcp` came to be named by generated
// code and boot warnings while no generator wrote them.

/// One `authz/<dir>/` bridge: the files, the paths that name it, and the crates
/// it needs. Plain fields — it is a `pub(super)` table, and a getter per field
/// would be one more place a row has to be read through.
pub(super) struct AuthzBridge {
    /// Folder under `crates/features/src/app_authz/`.
    pub dir: &'static str,
    /// The module type the `app_authz/mod.rs` index, the adapter and the app name.
    pub module: &'static str,
    /// How an adapter's own `module.rs` reaches it (inside the features crate).
    pub feature_path: &'static str,
    /// How an app's composition site reaches it.
    pub app_path: &'static str,
    /// `(file name, template)`, mirroring `demo/crates/features/src/app_authz/<dir>/`.
    pub files: &'static [(&'static str, &'static str)],
    /// What this bridge buys, printed as the run's next steps. Kept beside the
    /// files so the explanation and the code cannot drift apart.
    pub rationale: &'static [&'static str],
    /// Umbrella features the bridge's own source names.
    pub deps: &'static [&'static super::cargo::Dep],
    /// `g auth` writes this one as part of the base adapter, so an adapter
    /// generator must not re-create it. True for HTTP alone.
    pub written_by_g_auth: bool,
}

impl AuthzBridge {
    /// Already on disk — a second `g <transport>` must not re-create it.
    pub(super) fn exists(&self, ws: &NestrsWorkspace) -> bool {
        ws.features_root().join("app_authz").join(self.dir).is_dir()
    }

    /// The `app_authz/mod.rs` index lines this bridge adds — a `Vec` like
    /// [`lib_decls`], so a caller folds them into whichever single edit (or
    /// file body) they belong to.
    pub(super) fn decls(&self) -> Vec<String> {
        vec![
            format!("pub mod {};", self.dir),
            format!("pub use {}::{};", self.dir, self.module),
        ]
    }

    /// Queue the bridge's files.
    pub(super) fn queue(&self, s: &mut Scaffold, ws: &NestrsWorkspace) {
        let dir = ws.features_root().join("app_authz").join(self.dir);
        for (name, body) in self.files {
            s.create(dir.join(name), (*body).to_string());
        }
    }
}

/// The bridge that enforces `transport`, whoever writes it — `None` for the
/// transports that need none: **queue** and **schedule** have no caller to
/// authenticate, since a job runs on the app's own behalf.
pub(super) fn bridge_for(transport: Transport) -> Option<&'static AuthzBridge> {
    match transport {
        Transport::Http => Some(&HTTP_BRIDGE),
        Transport::Graphql => Some(&GRAPHQL_BRIDGE),
        Transport::Ws => Some(&WS_BRIDGE),
        Transport::Mcp => Some(&MCP_BRIDGE),
        Transport::Queue | Transport::Schedule => None,
    }
}

/// The base bridge: `AbilityGuard<AppAbility>` on the HTTP request, which every
/// other bridge re-runs. Written by `g auth`, not by `g http`.
static HTTP_BRIDGE: AuthzBridge = AuthzBridge {
    dir: "http",
    module: "AppAuthzHttpModule",
    feature_path: "crate::app_authz::AppAuthzHttpModule",
    app_path: "features::app_authz::AppAuthzHttpModule",
    files: &[
        ("mod.rs", auth::AUTHZ_HTTP_MOD),
        ("guard.rs", auth::AUTHZ_HTTP_GUARD),
        ("module.rs", auth::AUTHZ_HTTP_MODULE),
    ],
    rationale: &[
        "The guard runs on the HTTP request and attaches the caller's Ability, which",
        "every other transport's bridge re-runs. Controllers serving rows bind",
        "#[use_guards(AppAuthnGuard, AppAuthzGuard)] and import AppAuthzHttpModule.",
    ],
    deps: &[&super::cargo::AUTHZ],
    written_by_g_auth: true,
};

/// `/graphql` is one endpoint with no guard at the HTTP edge: authn and the
/// ability run **in band, per operation**, through a `GraphqlOperationGuard`.
/// Without these providers the endpoint falls back to a chain that installs no
/// ability at all, and every `#[authorize]` operation answers on rows nobody
/// scoped.
static GRAPHQL_BRIDGE: AuthzBridge = AuthzBridge {
    dir: "graphql",
    module: "AppAuthzGraphqlModule",
    feature_path: "crate::app_authz::AppAuthzGraphqlModule",
    app_path: "features::app_authz::AppAuthzGraphqlModule",
    files: &[
        ("mod.rs", auth::AUTHZ_GRAPHQL_MOD),
        ("bridge.rs", auth::AUTHZ_GRAPHQL_BRIDGE),
        ("module.rs", auth::AUTHZ_GRAPHQL_MODULE),
    ],
    rationale: &[
        "/graphql has no guard at the HTTP edge — authn and the ability run in band,",
        "per operation, through AppAuthzGraphqlModule. Every resolver serving rows",
        "imports it and declares #[authorize(Action, Entity)] or #[public].",
    ],
    deps: &[
        &super::cargo::AUTHZ,
        &super::cargo::SEAORM,
        &super::cargo::GRAPHQL,
    ],
    written_by_g_auth: false,
};

/// A WS upgrade is an HTTP GET, so a gateway reuses the HTTP guards rather than
/// a bridge of its own — what it needs is the `dyn SocketContext` carrying the
/// connection's data scope.
static WS_BRIDGE: AuthzBridge = AuthzBridge {
    dir: "ws",
    module: "AppAuthzWsModule",
    feature_path: "crate::app_authz::AppAuthzWsModule",
    app_path: "features::app_authz::AppAuthzWsModule",
    files: &[
        ("mod.rs", auth::AUTHZ_WS_MOD),
        ("module.rs", auth::AUTHZ_WS_MODULE),
    ],
    rationale: &[
        "A gateway reuses the HTTP guards — bind #[use_guards(AppAuthnGuard, AppAuthzGuard)]",
        "on the struct and import AppAuthzWsModule in the adapter's module.rs. It carries",
        "the dyn SocketContext that scopes the connection's rows to the caller.",
    ],
    deps: &[
        &super::cargo::AUTHZ,
        &super::cargo::SEAORM,
        &super::cargo::WS,
    ],
    written_by_g_auth: false,
};

/// `/mcp` gates in band, per operation. With no `McpOperationGuard` registered
/// it is **deny-all** — every tool call answers 401, which is the boot warning
/// `g mcp` prints and the state a reader following the docs used to land in.
static MCP_BRIDGE: AuthzBridge = AuthzBridge {
    dir: "mcp",
    module: "AppAuthzMcpModule",
    feature_path: "crate::app_authz::AppAuthzMcpModule",
    app_path: "features::app_authz::AppAuthzMcpModule",
    files: &[
        ("mod.rs", auth::AUTHZ_MCP_MOD),
        ("bridge.rs", auth::AUTHZ_MCP_BRIDGE),
        ("module.rs", auth::AUTHZ_MCP_MODULE),
    ],
    rationale: &[
        "/mcp denies every request until an McpOperationGuard is bound. AppAuthzMcpModule",
        "binds one: callers are authenticated and the ambient Ability is installed, so",
        "a tool can return entity rows through nest_rs::authz::masked_output_ambient.",
    ],
    deps: &[
        &super::cargo::AUTHZ,
        &super::cargo::SEAORM,
        &super::cargo::MCP,
    ],
    written_by_g_auth: false,
};

/// Append the HS256 dev secret unless the file already sets one — an app with
/// no `<PREFIX>_AUTHN__*` key material refuses to boot.
fn append_authn_secret(env_prefix: &str, rendered: String) -> crate::scaffold::Transform {
    // An empty key yields the namespace prefix `<PREFIX>_AUTHN__` — the same
    // join every real name uses, rather than a second hand-built one.
    let marker = crate::context::var_name(env_prefix, "AUTHN", "");
    Box::new(move |content: &str| {
        if content.contains(&marker) {
            return None;
        }
        Some(format!("{content}{rendered}"))
    })
}

fn print_next_steps(env_prefix: &str, wired: bool) {
    println!();
    println!("Next steps:");
    println!("  1. Add your rules in `crates/features/src/app_authz/ability.rs` — nothing is");
    println!("     granted until you do, so guarded routes answer 403.");
    if wired {
        println!(
            "  2. AppAuthnModule, AppAuthnHttpModule and AppAuthzHttpModule are wired into the"
        );
        println!("     current app.");
    } else {
        println!("  2. Import `features::app_authn::AppAuthnModule`,");
        println!("     `features::app_authn::AppAuthnHttpModule` and");
        println!("     `features::app_authz::AppAuthzHttpModule` in your app's `module.rs`.");
    }
    println!("  3. `POST /auth/dev-token` mints a bearer token to call your guarded routes");
    println!("     with. It refuses to boot outside development and test — delete");
    println!("     `crates/features/src/app_authn/http/` when you write the real login.");
    println!("  4. `.env` carries a development HS256 secret — replace it through the");
    println!("     real environment before deploying ({env_prefix}_AUTHN__SECRET).");
}
