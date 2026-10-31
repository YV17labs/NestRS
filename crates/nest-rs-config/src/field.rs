//! [`ConfigField`] — what a `#[config]` field reads from the environment, by
//! its type.

use crate::Result;
use crate::service::ConfigService;
use crate::setting::FromSetting;

/// A type a `#[config]` field holds, read from the environment under the
/// field's key: the deployment's variables over `base`, the value pinned in
/// code or the default.
///
/// `#[config]` writes [`Config::from_env`](crate::Config::from_env) one field at
/// a time through it, each key the field's name in SCREAMING_SNAKE — so the
/// struct is the whole contract, and no field is reachable from code alone.
/// A scalar ([`FromSetting`]) reads one variable, an `Option` of one keeps
/// `base` when it is unset, and a `Vec` of one reads a comma-separated list,
/// trimmed, empties dropped, each entry refused by its place. A vocabulary of
/// several variables implements it whole, its keys extending the field's with
/// `_` and each member's name (`TLS_RELOAD_SECS` for the `reload` of a field
/// `tls`): `__` is the namespace's separator, so a key never holds one. A
/// `ClientTls` (the `tls` feature's) alone names its material instead,
/// `TLS_CA_CERT`, `TLS_CERT` and `TLS_KEY`, the same in every namespace.
///
/// ```
/// use nest_rs_config::{ConfigField, ConfigService};
///
/// let env = ConfigService::with_vars("fixture", [("HOSTS", "a.example, b.example")]);
/// let hosts = Vec::<String>::overlay(&env, "HOSTS", Vec::new())?;
/// assert_eq!(hosts, ["a.example", "b.example"]);
/// assert_eq!(u16::overlay(&env, "PORT", 3000)?, 3000, "unset keeps the base");
/// # Ok::<(), nest_rs_config::ConfigError>(())
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is no type a `#[config]` field reads from the environment",
    label = "no `ConfigField` reads this field's type",
    note = "a scalar implements `FromSetting`; a `Duration` is read through its bounds, \
            `#[config(bounds = …)]`; a structured value as JSON, `#[config(json)]`; any other \
            field names its reader, `#[config(with = …)]`, or the struct writes its own \
            overlay, `#[config(from_env = \"manual\")]`"
)]
pub trait ConfigField: Sized {
    /// The variables under `key` over `base`: `base` where the deployment set
    /// none, and a value it set but that does not read refused naming the
    /// variable, never quoting it.
    fn overlay(env: &ConfigService, key: &str, base: Self) -> Result<Self>;
}

impl<T: FromSetting> ConfigField for T {
    fn overlay(env: &ConfigService, key: &str, base: Self) -> Result<Self> {
        match env.setting(key)? {
            Some(setting) => setting.read(),
            None => Ok(base),
        }
    }
}

impl<T: FromSetting> ConfigField for Option<T> {
    fn overlay(env: &ConfigService, key: &str, base: Self) -> Result<Self> {
        match env.setting(key)? {
            Some(setting) => setting.read().map(Some),
            None => Ok(base),
        }
    }
}

impl<T: FromSetting> ConfigField for Vec<T> {
    fn overlay(env: &ConfigService, key: &str, base: Self) -> Result<Self> {
        let Some(setting) = env.setting(key)? else {
            return Ok(base);
        };
        setting
            .value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .enumerate()
            .map(|(index, entry)| {
                T::from_setting(entry).map_err(|expected| {
                    setting.refuse(format_args!("entry {} of the list {expected}", index + 1))
                })
            })
            .collect()
    }
}

/// A structured value under `key` decoded as JSON over `base`: what
/// `#[config(json)]` expands to. Not API.
#[doc(hidden)]
pub fn overlay_json<T: serde::de::DeserializeOwned>(
    env: &ConfigService,
    key: &str,
    base: T,
) -> Result<T> {
    Ok(env.json(key)?.unwrap_or(base))
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;

    use nest_rs_core::Secret;

    use super::*;
    use crate::service::var_name;

    fn env(vars: &[(&str, &str)]) -> ConfigService {
        ConfigService::with_vars("fixture", vars.iter().copied())
    }

    #[test]
    fn a_scalar_takes_its_variable_and_keeps_the_base_when_unset() {
        let set = env(&[("PORT", "8080")]);
        assert_eq!(u16::overlay(&set, "PORT", 3000).unwrap(), 8080);
        assert_eq!(u16::overlay(&env(&[]), "PORT", 3000).unwrap(), 3000);
    }

    #[test]
    fn a_flag_reads_the_boolean_vocabulary() {
        for (text, read) in [("yes", true), ("OFF", false), ("1", true)] {
            assert_eq!(
                bool::overlay(&env(&[("ON", text)]), "ON", !read).unwrap(),
                read
            );
        }
        let refused = bool::overlay(&env(&[("ON", "maybe")]), "ON", false)
            .unwrap_err()
            .to_string();
        assert_eq!(
            refused,
            format!(
                "invalid value for {}: expected a boolean (1/0, true/false, yes/no, on/off)",
                var_name("fixture", "ON")
            )
        );
    }

    #[test]
    fn an_option_keeps_its_base_when_unset_and_holds_what_is_set() {
        assert_eq!(
            Option::<String>::overlay(&env(&[]), "NAME", Some("base".into())).unwrap(),
            Some("base".into())
        );
        assert_eq!(
            Option::<u32>::overlay(&env(&[("LIMIT", "7")]), "LIMIT", None).unwrap(),
            Some(7)
        );
    }

    #[test]
    fn a_list_refuses_an_entry_by_its_place_and_never_by_its_value() {
        let read = Vec::<IpAddr>::overlay(
            &env(&[("PROXIES", "10.0.0.1, ,10.0.0.2")]),
            "PROXIES",
            Vec::new(),
        )
        .unwrap();
        assert_eq!(read.len(), 2, "empties are dropped");
        let refused = Vec::<IpAddr>::overlay(
            &env(&[("PROXIES", "10.0.0.1,hunter2")]),
            "PROXIES",
            Vec::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            refused.contains("entry 2 of the list is not an IP address"),
            "{refused}"
        );
        assert!(!refused.contains("hunter2"), "{refused}");
    }

    #[test]
    fn a_secret_reads_its_value_and_never_shows_it() {
        let read =
            Secret::<String>::overlay(&env(&[("TOKEN", "hunter2")]), "TOKEN", Secret::default())
                .unwrap();
        assert_eq!(read.expose_secret(), "hunter2");
        assert!(!format!("{read:?}").contains("hunter2"));
    }

    #[test]
    fn a_duration_key_is_refused_whatever_the_field_type() {
        let refused = u64::overlay(&env(&[("WINDOW_SECS", "5")]), "WINDOW_SECS", 0);
        assert!(
            matches!(refused, Err(crate::ConfigError::UnboundedDuration { .. })),
            "{refused:?}"
        );
    }
}
