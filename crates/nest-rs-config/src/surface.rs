//! [`Surface`] — a development opening a config holds, resolved by the process
//! that uses it.

use std::borrow::Cow;

use crate::environment::Environment;
use crate::setting::FromSetting;

/// A setting a development run opens and a deployment does not: a public
/// surface (the documentation, a playground, a response header), plain HTTP, a
/// committed artifact's emit, a seed.
///
/// A field of this type holds it, [`ByProfile`](Self::ByProfile) in its
/// config's `Default`, so a pin over `..Default::default()` closes nothing it
/// does not name and opens nothing in a deployment. The consumer resolves it
/// where it uses it, the framework's own at the boot, with
/// [`is_open`](Self::is_open) or [`writes`](Self::writes). From the environment,
/// a boolean reads as [`Open`](Self::Open) or [`Closed`](Self::Closed); unset
/// keeps the base.
///
/// ```
/// use nest_rs_config::{Surface, config};
///
/// #[config(namespace = "reports")]
/// #[derive(Clone, Default)]
/// pub struct ReportsConfig {
///     /// Serve the sample reports, in a development run unless a deployment says so.
///     pub samples: Surface,
/// }
///
/// assert_eq!(ReportsConfig::default().samples, Surface::ByProfile);
/// assert!(Surface::Open.is_open());
/// assert!(!Surface::Closed.is_open());
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Surface {
    /// Whatever the process declares: open under a development or test profile,
    /// closed in a deployment and in a process that declares none.
    #[default]
    ByProfile,
    /// Open in every profile, the one way a deployment opens it in code.
    Open,
    /// Closed in every profile.
    Closed,
}

impl Surface {
    /// Whether it is open in this process: [`ByProfile`](Self::ByProfile) under
    /// a declared development or test profile alone.
    pub fn is_open(self) -> bool {
        self.resolved(|profile| !profile.is_deployed())
    }

    /// Whether the emit it gates writes its file in this process:
    /// [`ByProfile`](Self::ByProfile) under a declared development profile
    /// alone, since parallel suites would rewrite a committed file under test.
    pub fn writes(self) -> bool {
        self.resolved(|profile| profile == Environment::Development)
    }

    /// The deployed profile it is open in, if any: the one case a consumer says
    /// at `warn` where the opening takes effect, an opening the deployment wrote,
    /// honoured and visible. Under a development or test profile it is the
    /// surface doing its job, and in a deployment left [`ByProfile`](Self::ByProfile)
    /// it is closed.
    ///
    /// ```
    /// use nest_rs_config::Surface;
    ///
    /// assert_eq!(Surface::Closed.opened_in_deployment(), None);
    /// assert_eq!(Surface::ByProfile.opened_in_deployment(), None);
    /// ```
    pub fn opened_in_deployment(self) -> Option<Environment> {
        let environment = Environment::from_env();
        (environment.is_deployed() && self.is_open()).then_some(environment)
    }

    /// A written opening or closing, or what `by_profile` makes of the profile.
    fn resolved(self, by_profile: fn(Environment) -> bool) -> bool {
        match self {
            Self::ByProfile => by_profile(Environment::from_env()),
            Self::Open => true,
            Self::Closed => false,
        }
    }
}

/// A boolean, in the vocabulary [`ConfigService::flag`](crate::ConfigService::flag)
/// reads: true is [`Surface::Open`], false [`Surface::Closed`].
impl FromSetting for Surface {
    fn from_setting(text: &str) -> Result<Self, Cow<'static, str>> {
        Ok(if bool::from_setting(text)? {
            Self::Open
        } else {
            Self::Closed
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
mod tests {
    use super::*;
    use crate::{ConfigError, ConfigField as _, ConfigService};

    /// `read` under each profile a process can declare, `""` declaring none.
    fn by_profile(read: fn(Surface) -> bool, surface: Surface, expected: [bool; 6]) {
        figment::Jail::expect_with(|jail| {
            for (profile, expected) in [
                "development",
                "test",
                "staging",
                "production",
                "",
                "prodution",
            ]
            .into_iter()
            .zip(expected)
            {
                jail.set_env(Environment::var_name(), profile);
                assert_eq!(read(surface), expected, "{surface:?} under {profile:?}");
            }
            Ok(())
        });
    }

    #[test]
    fn by_profile_opens_under_development_and_test_alone() {
        by_profile(
            Surface::is_open,
            Surface::ByProfile,
            [true, true, false, false, false, false],
        );
    }

    #[test]
    fn an_emit_by_profile_writes_under_development_alone() {
        by_profile(
            Surface::writes,
            Surface::ByProfile,
            [true, false, false, false, false, false],
        );
    }

    #[test]
    fn a_written_value_holds_in_every_profile() {
        for read in [Surface::is_open, Surface::writes] {
            by_profile(read, Surface::Open, [true; 6]);
            by_profile(read, Surface::Closed, [false; 6]);
        }
    }

    #[test]
    fn the_default_is_the_profiles() {
        assert_eq!(Surface::default(), Surface::ByProfile);
    }

    const NAMESPACE: &str = "surface_test";

    fn read(vars: &[(&'static str, &'static str)], base: Surface) -> crate::Result<Surface> {
        let env = ConfigService::with_vars(NAMESPACE, vars.iter().copied());
        Surface::overlay(&env, "OPEN", base)
    }

    /// What a consumer says at `warn`: open, in a process that is deployed. The
    /// default is never that, and a development profile's opening is no incident.
    #[test]
    fn an_opening_outside_a_development_profile_is_what_a_consumer_says() {
        figment::Jail::expect_with(|jail| {
            for (profile, surface, opened) in [
                ("production", Surface::Open, Some(Environment::Production)),
                ("staging", Surface::Open, Some(Environment::Staging)),
                ("", Surface::Open, Some(Environment::Production)),
                ("production", Surface::ByProfile, None),
                ("production", Surface::Closed, None),
                ("development", Surface::Open, None),
                ("test", Surface::Open, None),
            ] {
                jail.set_env(Environment::var_name(), profile);
                assert_eq!(
                    surface.opened_in_deployment(),
                    opened,
                    "{surface:?} under {profile:?}"
                );
            }
            Ok(())
        });
    }

    #[test]
    fn a_boolean_reads_as_open_or_closed_and_unset_keeps_the_base() {
        for spelling in ["true", "1", "yes", "ON"] {
            assert_eq!(
                read(&[("OPEN", spelling)], Surface::Closed).expect("reads"),
                Surface::Open,
                "{spelling}"
            );
        }
        for spelling in ["false", "0", "no", "off"] {
            assert_eq!(
                read(&[("OPEN", spelling)], Surface::Open).expect("reads"),
                Surface::Closed,
                "{spelling}"
            );
        }
        for base in [Surface::ByProfile, Surface::Open, Surface::Closed] {
            assert_eq!(read(&[], base).expect("reads"), base);
        }
    }

    #[test]
    fn anything_else_is_refused_naming_the_variable_and_never_quoting_it() {
        let refused = read(&[("OPEN", "maybe")], Surface::ByProfile).expect_err("not a boolean");
        assert!(
            matches!(refused, ConfigError::Parse { ref var, .. }
                if *var == crate::var_name(NAMESPACE, "OPEN")),
            "{refused}"
        );
        assert!(!refused.to_string().contains("maybe"), "{refused}");
    }
}
