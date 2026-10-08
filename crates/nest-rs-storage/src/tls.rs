//! [`StorageTls`] — what an `https://` endpoint's certificate must chain to.

use std::fmt;

use nest_rs_config::{ConfigError, ConfigService, Result};

/// TLS trust for the endpoint. The default — nothing set — trusts the system's
/// store; the certificate is verified either way, and its name checked against
/// the endpoint's host.
///
/// Read once, when the configuration resolves: a renewed file takes effect at
/// the next boot.
#[derive(Clone, Default)]
pub struct StorageTls {
    /// PEM certificates of the authorities the store's certificate must chain
    /// to, **replacing** the system's — for a store a private authority signed.
    /// Read from `<PREFIX>_STORAGE__TLS_CA_CERT`, or the file
    /// `<PREFIX>_STORAGE__TLS_CA_CERT_FILE` names.
    pub ca_cert: Option<Vec<u8>>,
}

impl fmt::Debug for StorageTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageTls")
            .field("ca_cert", &self.ca_cert.as_ref().map(|_| "<pem>"))
            .finish()
    }
}

impl StorageTls {
    /// Overlay the deployment's TLS variables on `base`, refusing an authority
    /// file that holds no certificate — it would trust none — or one beside a
    /// plaintext endpoint, which would go unused.
    pub(crate) fn from_env(env: &ConfigService, base: Self, plaintext: bool) -> Result<Self> {
        let Some(pem) = env.material("TLS_CA_CERT")? else {
            return Ok(base);
        };
        if plaintext {
            return Err(pem.refuse(format!(
                "an authority is set for a plain-http endpoint, where no certificate is ever \
                 checked: point {} at an https:// endpoint, or unset the authority",
                env.var_name("ENDPOINT")
            )));
        }
        if authorities(&pem.value.bytes).is_none_or(|found| found.is_empty()) {
            return Err(
                pem.refuse("holds no PEM CERTIFICATE block, so it would trust no certificate")
            );
        }
        Ok(Self {
            ca_cert: Some(pem.value.bytes),
        })
    }
}

/// The authorities `pem` holds, `None` when it does not parse — said without
/// its content.
pub(crate) fn authorities(pem: &[u8]) -> Option<Vec<object_store::Certificate>> {
    object_store::Certificate::from_pem_bundle(pem).ok()
}

/// The refusal of an authority a hand-built config carries that holds no
/// certificate — the boot's check, for a config `from_env` never saw.
pub(crate) fn no_authority() -> ConfigError {
    ConfigError::parse(
        nest_rs_config::var_name("storage", "TLS_CA_CERT"),
        "holds no PEM CERTIFICATE block, so it would trust no certificate".to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> ConfigService {
        ConfigService::with_vars("storage", vars.iter().copied())
    }

    /// An authority is read for an `https://` endpoint, replacing the base.
    #[test]
    fn an_authority_is_read_for_an_encrypted_endpoint() {
        let authority = nest_rs_testing::TestAuthority::new();
        let tls = StorageTls::from_env(
            &env(&[("TLS_CA_CERT", authority.pem())]),
            StorageTls::default(),
            false,
        )
        .expect("an authority resolves");
        assert_eq!(tls.ca_cert.as_deref(), Some(authority.pem().as_bytes()));
        assert!(
            StorageTls::from_env(&env(&[]), StorageTls::default(), false)
                .expect("nothing set resolves")
                .ca_cert
                .is_none(),
            "and nothing set trusts the system's store"
        );
    }

    /// An authority beside a plaintext endpoint would go unused, and one
    /// holding no certificate would trust none: both are refused, naming the
    /// variable and never quoting what it holds.
    #[test]
    fn an_authority_that_cannot_be_used_is_refused_naming_its_variable() {
        let authority = nest_rs_testing::TestAuthority::new();
        let var = nest_rs_config::var_name("storage", "TLS_CA_CERT");
        let beside_plaintext = StorageTls::from_env(
            &env(&[("TLS_CA_CERT", authority.pem())]),
            StorageTls::default(),
            true,
        )
        .expect_err("an authority for a plaintext endpoint")
        .to_string();
        assert!(
            beside_plaintext.contains(&var) && beside_plaintext.contains("https://"),
            "{beside_plaintext}"
        );
        let empty = StorageTls::from_env(
            &env(&[("TLS_CA_CERT", "not a certificate s3cret")]),
            StorageTls::default(),
            false,
        )
        .expect_err("an authority holding no certificate")
        .to_string();
        assert!(
            empty.contains(&var)
                && empty.contains("no PEM CERTIFICATE")
                && !empty.contains("s3cret"),
            "{empty}"
        );
    }
}
