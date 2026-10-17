//! [`RedisInstance`] — the instance a connection serves, as its lines and its
//! refusals name it.

use std::borrow::Cow;

use nest_rs_config::{ConfigService, Namespaced, instance_namespace};
use nest_rs_core::Instance;

use crate::RedisConfig;
use crate::config::CONNECT_TIMEOUT;

/// The instance a connection was opened for: the name every line it files
/// carries as `instance` — empty for the default one — and the namespace its
/// variables are read under, which every refusal names them in.
#[derive(Clone, Debug)]
pub(crate) struct RedisInstance {
    name: &'static str,
    namespace: Cow<'static, str>,
}

impl RedisInstance {
    /// The instance `I`: `<PREFIX>_REDIS__*` for the default one,
    /// `<PREFIX>_REDIS__CACHE__*` for `Cache`.
    pub(crate) fn of<I: Instance>() -> Self {
        Self {
            name: I::NAME,
            namespace: instance_namespace::<I>(RedisConfig::NAMESPACE),
        }
    }

    /// The `instance` field of every line: the marker's name, empty for the
    /// default instance.
    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    /// The namespace the instance's [`RedisConfig`] is read under.
    pub(crate) fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The variable `key` is read from for this instance.
    pub(crate) fn var(&self, key: &str) -> String {
        nest_rs_config::var_name(&self.namespace, key)
    }

    /// Both spellings of `key`'s variable, inline and `_FILE`.
    pub(crate) fn spellings(&self, key: &str) -> String {
        nest_rs_config::spellings(&self.namespace, key)
    }

    /// The URL's variable, both spellings: what most refusals send a reader to.
    pub(crate) fn url_var(&self) -> String {
        self.spellings("URL")
    }

    /// The connect budget's variable.
    pub(crate) fn connect_timeout_var(&self) -> String {
        self.var(CONNECT_TIMEOUT.key())
    }

    /// The TLS material's three variables, listed.
    pub(crate) fn tls_vars(&self) -> String {
        format!(
            "{}, {} and {}",
            self.var("TLS_CA_CERT"),
            self.var("TLS_CERT"),
            self.var("TLS_KEY"),
        )
    }

    /// A reader of the instance's namespace, for the sentences [`ClientTls`]
    /// writes about it.
    ///
    /// [`ClientTls`]: nest_rs_config::ClientTls
    pub(crate) fn reader(&self) -> ConfigService {
        ConfigService::for_namespace(&self.namespace)
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Unnamed;

    use super::*;

    #[nest_rs_core::instance]
    struct Cache;

    #[test]
    fn the_default_instance_reads_the_crate_s_namespace_and_names_none() {
        let instance = RedisInstance::of::<Unnamed>();
        assert_eq!((instance.name(), instance.namespace()), ("", "redis"));
        assert_eq!(
            instance.connect_timeout_var(),
            nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS")
        );
    }

    #[test]
    fn a_named_instance_inserts_its_name_after_the_vendor() {
        let instance = RedisInstance::of::<Cache>();
        assert_eq!(
            (instance.name(), instance.namespace()),
            ("cache", "redis__cache")
        );
        assert_eq!(
            instance.url_var(),
            nest_rs_config::spellings("redis__cache", "URL")
        );
        assert_eq!(
            instance.reader().var_name("TLS_CA_CERT"),
            nest_rs_config::var_name("redis__cache", "TLS_CA_CERT")
        );
    }
}
