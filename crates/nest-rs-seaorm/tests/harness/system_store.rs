//! What both suites stand in for the system's certificate store, the one
//! `nest_rs_config::system_authorities` reads once per process.

use figment::Jail;

/// Makes `pem` the whole of the system's store inside `jail`: `SSL_CERT_FILE`
/// names a file holding it and `SSL_CERT_DIR` an empty folder, since cargo on
/// Linux hands every process it runs an `SSL_CERT_DIR` naming the platform's
/// folder, which the store reads beside the file.
pub(crate) fn stand_in_system_store(jail: &mut Jail, pem: &str) {
    let file = jail.directory().join("system-store.pem");
    std::fs::write(&file, pem).expect("the store's file");
    let folder = jail.directory().join("system-store");
    std::fs::create_dir(&folder).expect("the store's folder");
    jail.set_env("SSL_CERT_FILE", file.display());
    jail.set_env("SSL_CERT_DIR", folder.display());
}
