//! [`system_authorities`] — the certificate authorities the system trusts.

use std::sync::OnceLock;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// The certificate authorities the system trusts, as PEM: what every TLS
/// client the framework opens trusts unless its own setting names an
/// authority. An authority installed in the system's store — an enterprise's
/// private one, the dev container's — is trusted like any other, and
/// `SSL_CERT_FILE` / `SSL_CERT_DIR` point the read elsewhere, as for every
/// client honouring the system's store.
///
/// Read once per process: a client reconnecting never reads the store again,
/// and an authority installed after the boot is trusted from the next one. A
/// file the store names that cannot be read is said at `warn` and the rest is
/// kept; a store holding nothing trusts nothing, so every handshake is refused
/// as untrusted.
pub fn system_authorities() -> &'static [u8] {
    static READ: OnceLock<Vec<u8>> = OnceLock::new();
    READ.get_or_init(|| {
        let loaded = rustls_native_certs::load_native_certs();
        for error in &loaded.errors {
            tracing::warn!(
                target: crate::TARGET,
                error = %nest_rs_core::error_message(error),
                "a file of the system's certificate store could not be read; its authorities are not trusted",
            );
        }
        let mut pem = Vec::new();
        for cert in loaded.certs {
            pem.extend_from_slice(b"-----BEGIN CERTIFICATE-----\n");
            for line in STANDARD.encode(cert.as_ref()).as_bytes().chunks(64) {
                pem.extend_from_slice(line);
                pem.push(b'\n');
            }
            pem.extend_from_slice(b"-----END CERTIFICATE-----\n");
        }
        pem
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store is PEM a client parses whole: every block it holds decodes.
    #[test]
    fn the_system_store_reads_as_pem_certificates() {
        let pem = std::str::from_utf8(system_authorities()).expect("PEM is text");
        let blocks = pem.matches("-----BEGIN CERTIFICATE-----").count();
        assert_eq!(blocks, pem.matches("-----END CERTIFICATE-----").count());
        assert!(blocks > 0, "the dev container's store holds authorities");
        for body in pem.split("-----BEGIN CERTIFICATE-----\n").skip(1) {
            let body: String = body
                .split("-----END CERTIFICATE-----")
                .next()
                .unwrap_or_default()
                .lines()
                .collect();
            assert!(STANDARD.decode(body).is_ok(), "each block is base64");
        }
    }
}
