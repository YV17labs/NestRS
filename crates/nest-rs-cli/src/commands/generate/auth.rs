//! `nestrs g auth` — the app-side authn/authz adapter every guarded feature
//! imports: the `Claims` principal, `AuthnGuard`/`AuthnModule`, and
//! `AuthzAbility`/`AuthzGuard`/`AuthzModule`.
//!
//! The framework is generic over the principal and the policy, so these types
//! are app code; without them `#[use_guards(AuthnGuard, AuthzGuard)]` names two
//! types nothing defines.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

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
    let secrets = queue(&mut s, &ws, Vec::new())?;
    s.edit(
        ws.root.join("Cargo.toml"),
        ensure_workspace_deps(auth_deps()),
    );
    s.edit(ws.features_cargo(), ensure_features_deps(auth_deps()));
    s.edit(ws.features_lib(), ensure_lines(lib_decls()));
    let wired_app = wire(&ctx, &mut s);

    finish(s, opts.dry_run, &ws.root, "the auth adapter")?;
    print_next_steps(&secrets, wired_app.is_some());
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

/// Queue every auth file and the HS256 secrets — but none of the shared-file
/// edits ([`lib_decls`], [`auth_deps`]), which a caller folds into a single
/// `edit` per path. `authz_decls` are extra index lines for `authz/mod.rs`, which
/// is created here. Returns where the secrets went, for the run's closing lines.
pub(super) fn queue(
    s: &mut Scaffold,
    ws: &NestrsWorkspace,
    authz_decls: Vec<String>,
) -> CliResult<Secrets> {
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

    queue_secrets(s, &ws.root)
}

/// A file `g auth` draws a secret into, and the other files the cascade reads
/// in the same runs (`nest_rs_config`'s `.env.<env>.local`, `.env.local`,
/// `.env.<env>`, `.env`): a secret beside a key set in any of them fails the
/// boot, and beside another secret shadows it.
struct SecretFile {
    name: &'static str,
    template: &'static str,
    /// Whose secret it is, completing "`<name>` holds …".
    holds: &'static str,
    beside: &'static [&'static str],
}

static SECRET_FILES: [SecretFile; 2] = [
    SecretFile {
        name: ".env.local",
        template: auth::ENV_LOCAL_AUTHN,
        holds: "your development HS256 secret",
        beside: &[".env.development.local", ".env.development", ".env"],
    },
    SecretFile {
        name: ".env.test",
        template: auth::ENV_TEST_AUTHN,
        holds: "the test suites' HS256 secret",
        beside: &[".env.test.local", ".env"],
    },
];

/// What [`queue`] did with one secret file.
enum Secret {
    /// A secret drawn at random was appended to it.
    Drawn(&'static SecretFile),
    /// None was: `by`, the file itself or one read in the same runs, already
    /// sets a `<PREFIX>_AUTHN__` variable.
    Configured {
        file: &'static SecretFile,
        by: &'static str,
    },
}

/// Where [`queue`] put the HS256 secrets, or why it put none.
pub(super) struct Secrets {
    env_prefix: String,
    files: Vec<Secret>,
}

impl Secrets {
    /// The closing lines: each file's secret or the setting that kept it out,
    /// then what a deployment owes. Never a value.
    pub(super) fn lines(&self) -> Vec<String> {
        let namespace = crate::context::var_name(&self.env_prefix, "AUTHN", "");
        let mut lines: Vec<String> = self
            .files
            .iter()
            .map(|secret| match secret {
                Secret::Drawn(file) => format!("`{}` holds {}.", file.name, file.holds),
                Secret::Configured { file, by } if file.name == *by => {
                    format!("`{by}` already sets a {namespace} variable, so it got no secret.")
                }
                Secret::Configured { file, by } => format!(
                    "`{}` got no secret: `{by}`, read in the same runs, sets a {namespace} variable.",
                    file.name
                ),
            })
            .collect();
        lines.push(format!(
            "Set {} (or _FILE) in every deployed environment;",
            crate::context::var_name(&self.env_prefix, "AUTHN", "SECRET")
        ));
        lines.push("the boot refuses to start without key material.".to_owned());
        lines
    }
}

/// The HS256 key material: `.env`, which every environment reads, says where
/// the secrets live and holds none; each [`SECRET_FILES`] entry gets a secret
/// of its own unless a file of its runs already sets the namespace.
fn queue_secrets(s: &mut Scaffold, root: &Path) -> CliResult<Secrets> {
    // Rendered with this project's prefix, or the app never reads the secret.
    let env_prefix = crate::context::env_prefix();
    let render = |template: &str| template.replace("{{env_prefix}}", &env_prefix);
    // An empty key yields the namespace prefix `<PREFIX>_AUTHN__`.
    let marker = crate::context::var_name(&env_prefix, "AUTHN", "");

    for (file, template) in [
        (".env", auth::ENV_AUTHN),
        (".env.example", auth::ENV_EXAMPLE_AUTHN),
    ] {
        let path = root.join(file);
        let block = render(template);
        if !path.is_file() {
            s.create(path, block.trim_start().to_owned());
            continue;
        }
        let marker = marker.clone();
        // A comment counts here: these blocks are comments, so a mention is one
        // already there.
        s.edit(
            path,
            Box::new(move |content: &str| {
                (!content.contains(&marker)).then(|| format!("{content}{block}"))
            }),
        );
    }

    let mut files = Vec::with_capacity(SECRET_FILES.len());
    for file in &SECRET_FILES {
        if let Some(by) = configured_by(root, file, &marker)? {
            files.push(Secret::Configured { file, by });
            continue;
        }
        let block = render(file.template).replace("{{secret}}", &hs256_secret()?);
        let path = root.join(file.name);
        if path.is_file() {
            s.edit_secret(
                path,
                Box::new(move |content: &str| Some(format!("{content}{block}"))),
            );
        } else {
            s.create(path, block.trim_start().to_owned());
        }
        files.push(Secret::Drawn(file));
    }
    Ok(Secrets { env_prefix, files })
}

/// The first of `file` and the files read beside it that sets a `marker`
/// variable.
fn configured_by(
    root: &Path,
    file: &'static SecretFile,
    marker: &str,
) -> CliResult<Option<&'static str>> {
    for name in std::iter::once(file.name).chain(file.beside.iter().copied()) {
        if sets_namespace(&root.join(name), marker)? {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

/// Whether the file at `path` [`assigns_namespace`]; a missing one does not.
fn sets_namespace(path: &Path, marker: &str) -> CliResult<bool> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        read => {
            let contents = read.with_context(|| format!("cannot read `{}`", path.display()))?;
            Ok(assigns_namespace(&contents, marker))
        }
    }
}

/// Whether `contents` assigns a variable starting with `marker`, read line by
/// line as `nest_rs_config`'s loader reads it: a comment sets nothing.
fn assigns_namespace(contents: &str, marker: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        !line.starts_with('#')
            && line
                .split_once('=')
                .is_some_and(|(key, _)| key.trim().starts_with(marker))
    })
}

/// 256 bits from the OS's CSPRNG as 64 lowercase hex characters: RFC 7518 §3.2
/// wants an HS256 key of at least the hash's size.
fn hs256_secret() -> CliResult<String> {
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).context("the OS refused random bytes for the HS256 secret")?;
    Ok(key.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// What a generator that bootstraps the adapter on the way (`g resource`,
/// `g graphql` over a CRUD port) announces: the adapter, its secrets, and the
/// token route it serves with no credential.
pub(super) fn print_bootstrapped(secrets: &Secrets) {
    println!();
    println!("Also created the auth adapter (authn/, authz/) the guards need.");
    for line in secrets.lines() {
        println!("{line}");
    }
    println!();
    println!("It includes `POST /auth/dev-token`, which mints a token with no credential so");
    println!("your guarded routes are callable at once. It refuses to boot outside");
    println!("development and test. Import `features::authn::AuthnHttpModule` to serve it,");
    println!("or delete `crates/features/src/authn/http/` and write the real login route.");
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

fn print_next_steps(secrets: &Secrets, wired: bool) {
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
    for (i, line) in secrets.lines().iter().enumerate() {
        let lead = if i == 0 { "  4. " } else { "     " };
        println!("{lead}{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::assigns_namespace;

    const MARKER: &str = "ACME_AUTHN__";

    #[test]
    fn an_assignment_in_the_namespace_sets_it_as_the_loader_reads_it() {
        for line in [
            "ACME_AUTHN__SECRET=x",
            "  ACME_AUTHN__PUBLIC_KEY_FILE = /k.pub",
            "export ACME_AUTHN__JWKS_URI=https://issuer.example/jwks",
            "ACME_AUTHN__SECRET=",
        ] {
            assert!(
                assigns_namespace(&format!("A=1\n{line}\n"), MARKER),
                "{line}"
            );
        }
    }

    #[test]
    fn a_comment_a_mention_or_another_namespace_sets_nothing() {
        for line in [
            "# ACME_AUTHN__SECRET=x",
            "  # ACME_AUTHN__SECRET=",
            "ACME_SEAORM__URL=postgres://ACME_AUTHN__",
            "ACME_AUTHN__SECRET",
            "NESTRS_AUTHN__SECRET=x",
        ] {
            assert!(!assigns_namespace(&format!("{line}\n"), MARKER), "{line}");
        }
    }
}
