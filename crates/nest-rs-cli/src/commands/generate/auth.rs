//! `nestrs g auth` — the app-side authn/authz adapter every guarded feature
//! imports: the `Claims` principal, `AuthnGuard`/`AuthnModule`, and
//! `AppAbility`/`AuthzGuard`/`AuthzHttpModule`.
//!
//! The framework is generic over the principal and the policy, so these types
//! cannot ship in a `nest-rs-*` crate — they are app code, identical in every
//! project until you edit the rules. Generating them is what makes
//! `#[use_guards(AuthnGuard, AuthzGuard)]` resolve; without it a guarded
//! controller names two types nothing defines.

use std::path::PathBuf;

use super::cargo::{auth_deps, ensure_features_deps, ensure_workspace_deps};
use super::support::{finish, resolve_start, wire_into_app};
use crate::context::{Context, NestrsWorkspace};
use crate::error::{CliError, CliResult};
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
            "`{}` already has an auth adapter — edit `authz/ability.rs` to change the policy",
            ws.root.display()
        )));
    }

    let mut s = Scaffold::new();
    queue(&mut s, &ws);
    s.edit(
        ws.root.join("Cargo.toml"),
        ensure_workspace_deps(auth_deps()),
    );
    s.edit(ws.features_cargo(), ensure_features_deps(auth_deps()));
    s.edit(ws.features_lib(), ensure_lines(lib_decls()));
    let wired_app = wire(&ctx, &mut s);

    finish(s, opts.dry_run, &ws.root, "Created auth adapter")?;
    print_next_steps(wired_app.is_some());
    Ok(())
}

/// The `features` crate root declarations the adapter needs. Returned rather
/// than queued so a caller adding its own can fold them into **one**
/// `ensure_lines` — two `edit`s on the same file each re-read it from disk, and
/// the second write would clobber the first.
pub(super) fn lib_decls() -> Vec<String> {
    [
        "pub mod authn;",
        "pub mod authz;",
        "pub mod identity;",
        "pub use identity::{Claims, Role};",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Queue every auth file and the `.env` secret — but none of the shared-file
/// edits ([`lib_decls`], [`auth_deps`]), which a caller adding its own must
/// fold into a single `edit` per path. Split out so `g resource` can bootstrap
/// the adapter in the same transaction as the resource that needs it.
pub(super) fn queue(s: &mut Scaffold, ws: &NestrsWorkspace) {
    let src = ws.features_root();

    s.create(src.join("identity/mod.rs"), auth::IDENTITY_MOD.to_string());
    s.create(
        src.join("identity/claims.rs"),
        auth::IDENTITY_CLAIMS.to_string(),
    );

    s.create(src.join("authn/mod.rs"), auth::AUTHN_MOD.to_string());
    s.create(src.join("authn/module.rs"), auth::AUTHN_MODULE.to_string());
    s.create(
        src.join("authn/strategy.rs"),
        auth::AUTHN_STRATEGY.to_string(),
    );

    s.create(src.join("authz/mod.rs"), auth::AUTHZ_MOD.to_string());
    s.create(
        src.join("authz/ability.rs"),
        auth::AUTHZ_ABILITY.to_string(),
    );
    s.create(src.join("authz/module.rs"), auth::AUTHZ_MODULE.to_string());
    s.create(
        src.join("authz/http/mod.rs"),
        auth::AUTHZ_HTTP_MOD.to_string(),
    );
    s.create(
        src.join("authz/http/guard.rs"),
        auth::AUTHZ_HTTP_GUARD.to_string(),
    );
    s.create(
        src.join("authz/http/module.rs"),
        auth::AUTHZ_HTTP_MODULE.to_string(),
    );

    // Every scaffolded workspace has one; a hand-rolled tree may not, and a
    // missing `.env` is not a reason to refuse the whole adapter.
    let env = ws.root.join(".env");
    if env.is_file() {
        s.edit(env, append_authn_secret());
    } else {
        s.create(env, auth::ENV_AUTHN.trim_start().to_string());
    }
}

/// Both roots are listed at the composition site even though `AuthzHttpModule`
/// pulls `AuthnModule` in transitively — an app's `module.rs` is the inventory
/// of the concerns it serves. Returned rather than wired here so a caller
/// bootstrapping the adapter folds them into its own single `module.rs` edit.
pub(super) const APP_IMPORTS: [(&str, &str); 2] = [
    ("features::authn::AuthnModule", "AuthnModule"),
    ("features::authz::AuthzHttpModule", "AuthzHttpModule"),
];

fn wire(ctx: &Context, s: &mut Scaffold) -> Option<PathBuf> {
    wire_into_app(ctx, s, &APP_IMPORTS, None)
}

pub(super) fn exists(ws: &NestrsWorkspace) -> bool {
    ws.features_root().join("authz").is_dir()
}

/// Append the HS256 dev secret unless the file already sets one — an app with
/// no `NESTRS_AUTHN__*` key material refuses to boot.
fn append_authn_secret() -> crate::scaffold::Transform {
    Box::new(|content: &str| {
        if content.contains("NESTRS_AUTHN__") {
            return None;
        }
        Some(format!("{content}{}", auth::ENV_AUTHN))
    })
}

fn print_next_steps(wired: bool) {
    println!();
    println!("Next steps:");
    println!("  1. Add your rules in `crates/features/src/authz/ability.rs` — nothing is");
    println!("     granted until you do, so guarded routes answer 403.");
    if wired {
        println!("  2. AuthnModule + AuthzHttpModule are wired into the current app.");
    } else {
        println!("  2. Import `features::authn::AuthnModule` and");
        println!("     `features::authz::AuthzHttpModule` in your app's `module.rs`.");
    }
    println!("  3. `.env` carries a development HS256 secret — replace it through the");
    println!("     real environment before deploying (NESTRS_AUTHN__SECRET).");
}
