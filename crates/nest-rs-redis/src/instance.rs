//! [`RedisInstance`] — the instance a connection serves, as its lines and its
//! refusals name it.

use std::borrow::Cow;

use nest_rs_config::{ConfigService, Namespaced, instance_namespace};
use nest_rs_core::Instance;

use crate::RedisConfig;

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
        self.spellings(RedisConfig::URL.key())
    }

    /// The connect budget's variable.
    pub(crate) fn connect_timeout_var(&self) -> String {
        RedisConfig::CONNECT_TIMEOUT.var(&self.namespace)
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

/// `module` as an app imports it for the instance `I`: its bare path for the
/// default instance, typed with the marker for a named one —
/// `nest_rs::redis::RedisModule::<Cache>` — the import a remedy names.
pub(crate) fn typed_import<I: Instance>(module: &str) -> String {
    match I::NAME {
        "" => module.to_owned(),
        _ => format!(
            "{module}::<{}>",
            nest_rs_core::__private::short_type_name::<I>()
        ),
    }
}

/// The instance `I` as a remedy's sentence names it: the default one, or the
/// one its marker's name reads.
pub(crate) fn instance_phrase<I: Instance>() -> Cow<'static, str> {
    match I::NAME {
        "" => Cow::Borrowed("the default instance"),
        name => Cow::Owned(format!("the `{name}` instance")),
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::Unnamed;

    use super::*;

    #[nest_rs_core::instance]
    struct Cache;

    #[test]
    fn a_remedy_names_the_default_instance_s_import_bare_and_a_named_one_s_typed() {
        assert_eq!(
            typed_import::<Unnamed>("nest_rs::redis::RedisModule"),
            "nest_rs::redis::RedisModule"
        );
        assert_eq!(instance_phrase::<Unnamed>(), "the default instance");
        assert_eq!(
            typed_import::<Cache>("nest_rs::redis::RedisModule"),
            "nest_rs::redis::RedisModule::<Cache>"
        );
        assert_eq!(instance_phrase::<Cache>(), "the `cache` instance");
    }

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
