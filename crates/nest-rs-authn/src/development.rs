//! [`DevelopmentOnlyGuard`] — a route only a development or test process
//! serves, and the refused boot of any other process listing it.

use nest_rs_config::Environment;
use nest_rs_core::{Layer, UnitContext, async_trait, hooks, injectable};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::HttpView;

/// Serves a route only in a process that declares `development` or `test`
/// (`<PREFIX>_ENV`), and refuses the boot of any other process whose modules
/// list it: a route handing out what only a development run may — a token
/// minted with no credential, a seed — answers nowhere a deployment runs.
///
/// An environment nobody declared is production, so a process nobody told
/// refuses too. Outside development the boot fails at its module-init hooks,
/// naming the variable and the environment declared, never its raw value; a
/// request that still reaches the route — mounted from a module this app did
/// not list the guard in — is answered `403`, which the edge files on
/// [`TARGET`](crate::TARGET) with the reason `outside_development`.
///
/// ```
/// use nest_rs_authn::DevelopmentOnlyGuard;
/// use nest_rs_core::module;
/// use nest_rs_http::{Result, controller, routes};
///
/// #[controller(path = "/dev")]
/// #[use_guards(DevelopmentOnlyGuard)]
/// struct SeedController;
///
/// #[routes]
/// impl SeedController {
///     #[post("/seed")]
///     #[public]
///     async fn seed(&self) -> Result<()> {
///         Ok(())
///     }
/// }
///
/// #[module(providers = [DevelopmentOnlyGuard, SeedController])]
/// struct DevModule;
/// ```
#[injectable]
#[derive(Layer)]
pub struct DevelopmentOnlyGuard;

/// Whether a process that declares `declared` serves a development-only
/// route: `development` and `test` do, a deployment and the undeclared do not.
fn serves(declared: Option<Environment>) -> bool {
    declared.is_some_and(|environment| !environment.is_deployed())
}

/// Whether a process that declares `declared` boots with the guard listed:
/// where the route serves, or refused naming the variable and what it reads.
fn boots(declared: Option<Environment>) -> anyhow::Result<()> {
    if serves(declared) {
        return Ok(());
    }
    let reads = declared.map_or("none, so production", |environment| environment.as_str());
    Err(anyhow::anyhow!(
        "DevelopmentOnlyGuard guards a route only development and test serve, and {} declares \
         {reads}. Declare `development` or `test` for a development run, or remove the module \
         that lists the guard and its routes from this app.",
        Environment::var_name(),
    ))
}

#[hooks]
impl DevelopmentOnlyGuard {
    #[on_module_init]
    async fn refuse_outside_development(&self) -> anyhow::Result<()> {
        boots(Environment::declared())
    }
}

#[async_trait]
impl Guard for DevelopmentOnlyGuard {
    async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
        if serves(Environment::declared()) {
            return Ok(());
        }
        Err(Denial::forbidden("development-only route").with_reason("outside_development"))
    }

    fn denial_statuses(&self, _public: bool) -> &'static [u16] {
        &[403]
    }

    fn target(&self) -> &'static str {
        crate::TARGET
    }
}

impl GuardFor<HttpView> for DevelopmentOnlyGuard {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn development_and_test_serve_and_nothing_else_does() {
        for (declared, serving) in [
            (Some(Environment::Development), true),
            (Some(Environment::Test), true),
            (Some(Environment::Staging), false),
            (Some(Environment::Production), false),
            (None, false),
        ] {
            assert_eq!(serves(declared), serving, "{declared:?}");
            assert_eq!(boots(declared).is_ok(), serving, "{declared:?}");
        }
    }

    #[test]
    fn the_refusal_names_the_variable_and_the_environment_declared() {
        let staging = boots(Some(Environment::Staging))
            .expect_err("refused")
            .to_string();
        assert!(staging.contains(&Environment::var_name()), "{staging}");
        assert!(staging.contains("declares staging"), "{staging}");

        let undeclared = boots(None).expect_err("refused").to_string();
        assert!(
            undeclared.contains("declares none, so production"),
            "{undeclared}"
        );
    }
}
