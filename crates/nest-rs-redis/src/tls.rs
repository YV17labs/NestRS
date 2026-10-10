//! What a `rediss://` connection trusts and presents — the shared
//! [`ClientTls`] read under `<PREFIX>_REDIS__TLS_*` — handed to `redis`, and
//! what a refusal of it means.
//!
//! `rediss://` is TLS, `redis://` is plaintext. A TLS connection verifies
//! Redis's certificate for the URL's host against the authorities
//! [`ClientTls::authorities_pem`] hands over — the system's, read once, unless
//! the deployment names others: `redis` itself would read the store again for
//! every connection, blocking the runtime.
//!
//! **Verification is never switched off.** `rediss://…#insecure` is refused at
//! boot, and the client is built without `redis`'s `tls-rustls-insecure`.

use std::time::Duration;

use nest_rs_config::{ClientTls, ConfigService, Namespaced};
use redis::aio::MultiplexedConnection;

use crate::topology::Hello;
use crate::{RedisConfig, RedisError};

/// The reader whose namespace every Redis TLS sentence names.
fn names() -> ConfigService {
    ConfigService::for_namespace(RedisConfig::NAMESPACE)
}

/// The material every connection a URL opens is built with: `None` over
/// plaintext. Material beside a plaintext URL is refused, since it would go
/// unused, and so is material no handshake could use — before anything is
/// dialled.
pub(crate) fn material(
    tls: &ClientTls,
    encrypted: bool,
    endpoint: &str,
) -> Result<Option<redis::TlsCertificates>, RedisError> {
    if !encrypted {
        if !tls.is_empty() {
            return Err(RedisError::PlaintextUrl {
                endpoint: endpoint.to_owned(),
            });
        }
        return Ok(None);
    }
    tls.check(&names())
        .map_err(|refused| RedisError::TlsRefused {
            endpoint: endpoint.to_owned(),
            reason: refused.to_string(),
            source: None,
        })?;
    // `redis` builds its client from the process's default provider.
    nest_rs_config::crypto_provider();
    Ok(Some(redis::TlsCertificates {
        client_tls: tls
            .identity_pem()
            .map(|(cert, key)| redis::ClientTlsConfig {
                client_cert: cert.to_vec(),
                client_key: key.to_vec(),
            }),
        root_cert: Some(tls.authorities_pem().to_vec()),
    }))
}

/// Say `endpoint`'s TLS refusal at `warn` — the one line every topology says
/// it in, with what to change.
pub(crate) fn say_refused(endpoint: &str, error: &redis::RedisError) {
    tracing::warn!(
        target: crate::TARGET,
        endpoint = %endpoint,
        reason = %remedy(error),
        error = %nest_rs_core::error_message(error),
        "redis refused a reopened tls connection",
    );
}

/// The hosts refusing TLS, each said once until a connection to it opens.
static REFUSING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

/// Say what opening a connection to `endpoint` met, when TLS refused it: once,
/// until a connection to it opens again.
pub(crate) fn observe_refusal<T>(
    endpoint: &str,
    opened: &std::result::Result<T, redis::RedisError>,
) {
    let mut refusing = REFUSING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match opened {
        Ok(_) => {
            refusing.remove(endpoint);
        }
        Err(error) if negotiation_failed(error) => {
            if refusing.insert(endpoint.to_owned()) {
                say_refused(endpoint, error);
            }
        }
        Err(_) => {}
    }
}

/// What the server behind a connection `client` just opened says of itself.
///
/// `HELLO` is the first command a link sends after its dial, and a refusal of
/// the client's certificate lands right there: TLS 1.3 settles the
/// certificate after the client's side of the handshake, and `redis` drops a
/// refusal that lands before a command is in flight, so `HELLO` would meet a
/// closed connection. A handshake of its own then hears the refusal.
pub(crate) async fn hello(
    client: &redis::Client,
    connection: &mut MultiplexedConnection,
    budget: Duration,
) -> Result<Hello, redis::RedisError> {
    match Hello::ask(connection).await {
        Err(dropped) if dropped.is_connection_dropped() => {
            Err(refusal(client, budget).await.unwrap_or(dropped))
        }
        answered => answered,
    }
}

/// Redis's TLS refusal of a handshake of `client`'s, heard in order — `None`
/// over plaintext, when Redis accepts it, or when it fails some other way or
/// not within `budget`, and the caller's own error stands.
///
/// TLS 1.3 refuses a client certificate after the client's side of the
/// handshake is over, so the refusal is the first record Redis sends. `redis`'s
/// multiplexed connection drops one it reads before a command is in flight
/// (`PipelineSink::send_result` has no caller to hand it to) and its callers
/// meet a closed connection; its blocking connection writes before it reads.
pub(crate) async fn refusal(client: &redis::Client, budget: Duration) -> Option<redis::RedisError> {
    if !matches!(
        client.get_connection_info().addr(),
        redis::ConnectionAddr::TcpTls { .. }
    ) {
        return None;
    }
    let client = client.clone();
    let heard = tokio::task::spawn_blocking(move || {
        let mut connection = client.get_connection_with_timeout(budget)?;
        connection.set_write_timeout(Some(budget))?;
        connection.set_read_timeout(Some(budget))?;
        redis::cmd("PING").query::<()>(&mut connection)
    });
    match tokio::time::timeout(budget, heard).await {
        Ok(Ok(Err(refused))) if negotiation_failed(&refused) => Some(refused),
        Ok(Err(failed)) if failed.is_panic() => std::panic::resume_unwind(failed.into_panic()),
        _ => None,
    }
}

/// Why the client refused material [`ClientTls::check`] let through: both
/// parse the same PEM with the same parsers, so this is a client the check
/// did not foresee.
pub(crate) fn unusable_material() -> String {
    let names = names();
    format!(
        "{}, {} and {} could not build a client, although they passed the boot's check",
        names.spellings("TLS_CA_CERT"),
        names.spellings("TLS_CERT"),
        names.spellings("TLS_KEY"),
    )
}

/// Whether `error` is a TLS negotiation every attempt would fail the same way.
pub(crate) fn negotiation_failed(error: &(dyn std::error::Error + 'static)) -> bool {
    ClientTls::negotiation_failed(error)
}

/// What to change about a failed negotiation, naming the setting; rustls's own
/// reason travels as the error's source.
pub(crate) fn remedy(error: &(dyn std::error::Error + 'static)) -> String {
    ClientTls::remedy(&names(), "Redis", "URL", error)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{AUTHORITY, mutual_tls_listener, trusting_the_authority};

    #[test]
    fn a_tls_clients_material_leaves_a_process_default_provider_installed() {
        assert!(rustls::crypto::CryptoProvider::get_default().is_none());
        material(&ClientTls::default(), true, "redis.internal:6380")
            .expect("nothing set is the system's authorities");
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "a provider is installed for `redis` to build with"
        );
    }

    #[test]
    fn the_material_reaches_the_client_as_set() {
        let Ok(Some(certificates)) = material(&trusting_the_authority(), true, "x:1") else {
            panic!("an authority is material for an encrypted URL");
        };
        assert_eq!(
            certificates.root_cert.as_deref(),
            Some(AUTHORITY.pem().as_bytes())
        );
        assert!(certificates.client_tls.is_none());
        assert!(
            matches!(material(&ClientTls::default(), false, "x:1"), Ok(None)),
            "and plaintext carries none"
        );
    }

    /// The deterministic witness of the arm every link's first `HELLO` takes:
    /// the refusal is let land before the first command is sent, as a server
    /// faster than the client lands it. That command meets a closed
    /// connection — `redis`'s drop, held to the client this crate links — and
    /// [`hello`] hears the refusal all the same.
    #[tokio::test]
    async fn the_first_hello_hears_a_client_certificate_refused_before_it_was_sent() {
        let (addr, serving) = mutual_tls_listener().await;
        let url = format!("rediss://{addr}/");
        let Ok(crate::url::RedisUrl::Standalone(info)) = crate::url::RedisUrl::parse(&url) else {
            panic!("{url} is a URL of one server");
        };
        let budget = Duration::from_secs(2);
        let endpoint = addr.to_string();
        let certificates =
            material(&trusting_the_authority(), true, &endpoint).expect("the authority is usable");
        let client = crate::connection::open_client(info, certificates.as_ref(), &endpoint, budget)
            .expect("a client opens");
        let landed = || async {
            let connection = crate::connection::dial(&client, budget)
                .await
                .expect("the client's side of the handshake completes");
            tokio::time::sleep(Duration::from_millis(200)).await;
            connection
        };

        let met = Hello::ask(&mut landed().await).await;
        let heard = hello(&client, &mut landed().await, budget).await;
        serving.abort();

        let Err(met) = met else {
            panic!("a Redis refusing the client's certificate answers nothing");
        };
        assert!(
            met.is_connection_dropped() && !negotiation_failed(&met),
            "the first command meets a dropped connection, not the refusal: {met}",
        );
        let Err(refused) = heard else {
            panic!("a Redis refusing the client's certificate answers nothing");
        };
        assert!(negotiation_failed(&refused), "{refused}");
        assert!(
            remedy(&refused).contains("requires a client certificate"),
            "{refused}"
        );
    }
}
