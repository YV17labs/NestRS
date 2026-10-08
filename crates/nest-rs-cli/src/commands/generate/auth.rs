//! `nestrs g auth` — the app-side authn/authz adapter every guarded feature
//! imports: the `Claims` principal, `AuthnGuard`/`AuthnModule`, and
//! `AuthzAbility`/`AuthzGuard`/`AuthzModule`.
//!
//! The framework is generic over the principal and the policy, so these types
//! are app code; without them `#[use_guards(AuthnGuard, AuthzGuard)]` names two
//! types nothing defines.

use std::path::PathBuf;

use super::cargo::{auth_deps, ensure_features_deps, ensure_workspace_deps};
use super::support::{finish, wire_into_app};
use crate::commands::resolve_start;
use crate::context::{Context, NestrsWorkspace};
use crate::error::{CliError, CliResult};
use crate::naming::Transport;
use crate::scaffold::{Scaffold, ensure_lines};
use crate::templates::auth;

pub(crate) struct AuthOptions {
    pub path: Option<PathBuf>,
    pub dry_run: bool,
}

pub(crate) fn run(opts: AuthOptions) -> CliResult<()> {
    let ctx = Context::detect(&resolve_start(opts.path))?;
    let ws = ctx.workspace.clone().ok_or(CliError::NotNestrsWorkspace)?;

    if exists(&ws) {
        return Err(CliError::Anyhow(anyhow::anyhow!(
            "`{}` already has an auth adapter — edit `authz/ability.rs` to change the policy",
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

/// The `features` crate root declarations the adapter needs, for a caller to
/// fold into **one** `ensure_lines`: a second `edit` on the same file would
/// clobber the first.
pub(super) fn lib_decls() -> Vec<String> {
    [
        "pub mod authn;",
        "pub mod authz;",
        "pub use authn::{Claims, Role};",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Queue every auth file and the `.env` secret — but none of the shared-file
/// edits ([`lib_decls`], [`auth_deps`]), which a caller folds into a single
/// `edit` per path. `authz_decls` are extra index lines for `authz/mod.rs`, which
/// is created here.
pub(super) fn queue(s: &mut Scaffold, ws: &NestrsWorkspace, authz_decls: Vec<String>) {
    let src = ws.features_root();

    const FILES: [(&str, &str); 12] = [
        ("authn/claims.rs", auth::AUTHN_CLAIMS),
        ("authn/mod.rs", auth::AUTHN_MOD),
        ("authn/module.rs", auth::AUTHN_MODULE),
        ("authn/strategy.rs", auth::AUTHN_STRATEGY),
        ("authn/http/mod.rs", auth::AUTHN_HTTP_MOD),
        ("authn/http/audit.rs", auth::AUTHN_HTTP_AUDIT),
        ("authn/http/guard.rs", auth::AUTHN_HTTP_GUARD),
        ("authn/http/controller.rs", auth::AUTHN_HTTP_CONTROLLER),
        ("authn/http/module.rs", auth::AUTHN_HTTP_MODULE),
        ("authz/ability.rs", auth::AUTHZ_ABILITY),
        ("authz/guard.rs", auth::AUTHZ_GUARD),
        ("authz/module.rs", auth::AUTHZ_MODULE),
    ];
    for (path, body) in FILES {
        s.create(src.join(path), body.to_string());
    }

    let authz_mod =
        ensure_lines(authz_decls)(auth::AUTHZ_MOD).unwrap_or_else(|| auth::AUTHZ_MOD.to_string());
    s.create(src.join("authz/mod.rs"), authz_mod);

    // Rendered with this project's prefix, or the app never reads the secret.
    let env_prefix = crate::context::env_prefix();
    let env_authn = auth::ENV_AUTHN.replace("{{env_prefix}}", &env_prefix);
    let env = ws.root.join(".env");
    if env.is_file() {
        s.edit(env, append_authn_secret(&env_prefix, env_authn));
    } else {
        s.create(env, env_authn.trim_start().to_string());
    }
}

/// The app imports `g auth` wires: both roots, though `AuthzModule` pulls
/// `AuthnModule` in transitively — an app's `module.rs` is the inventory of what
/// it serves.
pub(super) fn app_imports() -> [(&'static str, &'static str); 3] {
    [
        ("features::authn::AuthnModule", "AuthnModule"),
        ("features::authn::AuthnHttpModule", "AuthnHttpModule"),
        (HTTP_BRIDGE.app_path, HTTP_BRIDGE.module),
    ]
}

fn wire(ctx: &Context, s: &mut Scaffold) -> Option<PathBuf> {
    wire_into_app(ctx, s, &app_imports(), None)
}

pub(super) fn exists(ws: &NestrsWorkspace) -> bool {
    ws.features_root().join("authz").is_dir()
}

/// One `authz/<dir>/` bridge: the files, the paths that name it, and the crates
/// it needs.
pub(super) struct AuthzBridge {
    /// Folder under `crates/features/src/authz/`. Empty for the base bridge,
    /// whose providers sit at the `authz/` root — see [`HTTP_BRIDGE`].
    pub dir: &'static str,
    /// The module type the `authz/mod.rs` index, the adapter and the app name.
    pub module: &'static str,
    /// How an adapter's own `module.rs` reaches it (inside the features crate).
    pub feature_path: &'static str,
    /// How an app's composition site reaches it.
    pub app_path: &'static str,
    /// `(file name, template)`, mirroring `demo/crates/features/src/authz/<dir>/`.
    pub files: &'static [(&'static str, &'static str)],
    /// What this bridge buys, printed as the run's next steps.
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
        ws.features_root().join("authz").join(self.dir).is_dir()
    }

    /// The `authz/mod.rs` index lines this bridge adds, for a caller to fold into
    /// its single edit.
    pub(super) fn decls(&self) -> Vec<String> {
        vec![
            format!("pub mod {};", self.dir),
            format!("pub use {}::{};", self.dir, self.module),
        ]
    }

    /// Queue the bridge's files.
    pub(super) fn queue(&self, s: &mut Scaffold, ws: &NestrsWorkspace) {
        let dir = ws.features_root().join("authz").join(self.dir);
        for (name, body) in self.files {
            s.create(dir.join(name), (*body).to_string());
        }
    }
}

/// The bridge that enforces `transport`, whoever writes it — `None` for the
/// transports that need none: **queue**, **schedule** and **events** have no
/// caller to authenticate, since their work runs on the app's own behalf.
pub(super) fn bridge_for(transport: Transport) -> Option<&'static AuthzBridge> {
    match transport {
        Transport::Http => Some(&HTTP_BRIDGE),
        Transport::Graphql => Some(&GRAPHQL_BRIDGE),
        Transport::Ws => Some(&WS_BRIDGE),
        Transport::Mcp => Some(&MCP_BRIDGE),
        Transport::Queue | Transport::Schedule | Transport::Events => None,
    }
}

/// The base bridge, with no folder of its own: `AbilityGuard` answers every
/// transport, so its guard and `AuthzModule` sit at the `authz/` root, written
/// by `g auth` itself. `dir` and `files` are empty and never read.
static HTTP_BRIDGE: AuthzBridge = AuthzBridge {
    dir: "",
    module: "AuthzModule",
    feature_path: "crate::authz::AuthzModule",
    app_path: "features::authz::AuthzModule",
    files: &[],
    rationale: &[
        "The guard attaches the caller's Ability and answers every transport, so it is",
        "provided by AuthzModule itself. Controllers serving rows bind",
        "#[use_guards(AuthnGuard, AuthzGuard)] and import AuthzModule.",
    ],
    deps: &[&super::cargo::AUTHZ],
    written_by_g_auth: true,
};

/// `/graphql` is one endpoint with no guard at the HTTP edge: authn and the
/// ability run **in band, per operation**, through a `GraphqlOperationGuard`;
/// without it no ability is installed.
static GRAPHQL_BRIDGE: AuthzBridge = AuthzBridge {
    dir: "graphql",
    module: "AuthzGraphqlModule",
    feature_path: "crate::authz::AuthzGraphqlModule",
    app_path: "features::authz::AuthzGraphqlModule",
    files: &[
        ("mod.rs", auth::AUTHZ_GRAPHQL_MOD),
        ("bridge.rs", auth::AUTHZ_GRAPHQL_BRIDGE),
        ("module.rs", auth::AUTHZ_GRAPHQL_MODULE),
    ],
    rationale: &[
        "/graphql has no guard at the HTTP edge — authn and the ability run in band,",
        "per operation, through AuthzGraphqlModule. Every resolver serving rows",
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
    module: "AuthzWsModule",
    feature_path: "crate::authz::AuthzWsModule",
    app_path: "features::authz::AuthzWsModule",
    files: &[
        ("mod.rs", auth::AUTHZ_WS_MOD),
        ("module.rs", auth::AUTHZ_WS_MODULE),
    ],
    rationale: &[
        "A gateway reuses the HTTP guards — bind #[use_guards(AuthnGuard, AuthzGuard)]",
        "on the struct and import AuthzWsModule in the adapter's module.rs. It carries",
        "the dyn SocketContext that scopes the connection's rows to the caller.",
    ],
    deps: &[
        &super::cargo::AUTHZ,
        &super::cargo::SEAORM,
        &super::cargo::WS,
    ],
    written_by_g_auth: false,
};

/// `/mcp` gates in band, per operation; with no `McpOperationGuard` registered
/// it is **deny-all**.
static MCP_BRIDGE: AuthzBridge = AuthzBridge {
    dir: "mcp",
    module: "AuthzMcpModule",
    feature_path: "crate::authz::AuthzMcpModule",
    app_path: "features::authz::AuthzMcpModule",
    files: &[
        ("mod.rs", auth::AUTHZ_MCP_MOD),
        ("bridge.rs", auth::AUTHZ_MCP_BRIDGE),
        ("module.rs", auth::AUTHZ_MCP_MODULE),
    ],
    rationale: &[
        "/mcp denies every request until an McpOperationGuard is bound. AuthzMcpModule",
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
    // An empty key yields the namespace prefix `<PREFIX>_AUTHN__`.
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
    println!("  1. Add your rules in `crates/features/src/authz/ability.rs` — nothing is");
    println!("     granted until you do, so guarded routes answer 403.");
    if wired {
        println!("  2. AuthnModule, AuthnHttpModule and AuthzModule are wired into the");
        println!("     current app.");
    } else {
        println!("  2. Import `features::authn::AuthnModule`,");
        println!("     `features::authn::AuthnHttpModule` and");
        println!("     `features::authz::AuthzModule` in your app's `module.rs`.");
    }
    println!("  3. `POST /auth/dev-token` mints a bearer token to call your guarded routes");
    println!("     with. It refuses to boot outside development and test — delete");
    println!("     `crates/features/src/authn/http/` when you write the real login.");
    println!("  4. `.env` carries a development HS256 secret — replace it through the");
    println!("     real environment before deploying ({env_prefix}_AUTHN__SECRET).");
}
