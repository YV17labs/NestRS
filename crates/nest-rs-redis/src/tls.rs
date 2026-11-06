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

use nest_rs_config::{ClientTls, Environment};
use redis::aio::{ConnectionLike, MultiplexedConnection};

use crate::RedisError;
use crate::instance::RedisInstance;
use crate::topology::Hello;
use crate::url::RedisUrl;

/// What the boot says when a deployment reaches Redis in plaintext.
pub(crate) const PLAINTEXT_OPEN: &str = "Redis is reached in plaintext outside a dev profile: its \
     credentials and its data travel unencrypted";

/// One `warn` when a deployment reaches `instance` in plaintext off this
/// machine: honoured, since its URL wrote it, and said by the one call every
/// topology opens through, once per instance, where a client rebuilt or a link
/// reopened never comes back.
pub(crate) fn warn_when_open(instance: &RedisInstance, url: &RedisUrl) {
    let Some(endpoint) = url.plaintext_off_this_machine() else {
        return;
    };
    let environment = Environment::from_env();
    if environment.is_deployed() {
        tracing::warn!(
            target: crate::TARGET,
            instance = instance.name(),
            environment = environment.as_str(),
            endpoint = %endpoint,
            "{PLAINTEXT_OPEN}",
        );
    }
}

/// The material every connection `instance`'s URL opens is built with: `None`
/// over plaintext. Material beside a plaintext URL is refused, since it would
/// go unused, and so is material no handshake could use — before anything is
/// dialled; each refusal names the instance's variables.
pub(crate) fn material(
    instance: &RedisInstance,
    tls: &ClientTls,
    encrypted: bool,
    endpoint: &str,
) -> Result<Option<redis::TlsCertificates>, RedisError> {
    if !encrypted {
        if !tls.is_empty() {
            return Err(RedisError::PlaintextUrl {
                endpoint: endpoint.to_owned(),
                url_var: instance.url_var(),
                tls_vars: instance.tls_vars(),
            });
        }
        return Ok(None);
    }
    tls.check(&instance.reader())
        .map_err(|refused| RedisError::TlsRefused {
            endpoint: endpoint.to_owned(),
            reason: refused.to_string(),
            source: None,
        })?;
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

/// Say `instance`'s TLS refusal by `endpoint` at `warn` — the one line every
/// topology says it in, with what to change.
pub(crate) fn say_refused(instance: &RedisInstance, endpoint: &str, error: &redis::RedisError) {
    tracing::warn!(
        target: crate::TARGET,
        instance = instance.name(),
        endpoint = %endpoint,
        reason = %remedy(instance, error),
        error = %nest_rs_core::error_message(error),
        "redis refused a reopened tls connection",
    );
}

/// The hosts refusing TLS, each said once until a connection to it opens.
static REFUSING: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

/// Say what opening a connection of `instance`'s to `endpoint` met, when TLS
/// refused it: once, until a connection to it opens again.
pub(crate) fn observe_refusal<T>(
    instance: &RedisInstance,
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
        Err(error) if ClientTls::negotiation_failed(error) => {
            if refusing.insert(endpoint.to_owned()) {
                say_refused(instance, endpoint, error);
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
/// closed connection. A handshake of its own, within `budget`, then hears the
/// refusal.
pub(crate) async fn hello(
    client: &redis::Client,
    connection: &mut MultiplexedConnection,
    budget: Duration,
) -> Result<Hello, redis::RedisError> {
    match asked(connection, budget).await {
        Err(dropped) if dropped.is_connection_dropped() => {
            Err(refusal(client, budget).await.unwrap_or(dropped))
        }
        answered => answered,
    }
}

/// What `HELLO` says on `connection`, asked again at each quarter of `budget`
/// it waits unanswered; the caller bounds the whole wait.
///
/// `redis` sends a command through a Tokio channel whose reserved send succeeds
/// after the receiver is gone (`mpsc::Permit::send`), so one sent as the
/// connection's driver ends waits on an answer nothing will give, and only the
/// next meets the closed connection.
async fn asked(
    connection: &mut impl ConnectionLike,
    budget: Duration,
) -> Result<Hello, redis::RedisError> {
    loop {
        if let Ok(answered) = tokio::time::timeout(budget / 4, Hello::ask(&mut *connection)).await {
            return answered;
        }
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
        Ok(Ok(Err(refused))) if ClientTls::negotiation_failed(&refused) => Some(refused),
        Ok(Err(failed)) if failed.is_panic() => std::panic::resume_unwind(failed.into_panic()),
        _ => None,
    }
}

/// Why the client refused material [`ClientTls::check`] let through: both
/// parse the same PEM with the same parsers, so this is a client the check
/// did not foresee.
pub(crate) fn unusable_material(instance: &RedisInstance) -> String {
    format!(
        "{}, {} and {} could not build a client, although they passed the boot's check",
        instance.spellings("TLS_CA_CERT"),
        instance.spellings("TLS_CERT"),
        instance.spellings("TLS_KEY"),
    )
}

/// What to change about a failed negotiation of `instance`'s, naming its
/// setting; rustls's own reason travels as the error's source.
pub(crate) fn remedy(
    instance: &RedisInstance,
    error: &(dyn std::error::Error + 'static),
) -> String {
    ClientTls::remedy(
        &instance.reader(),
        "Redis",
        crate::RedisConfig::URL.key(),
        error,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::testing::{AUTHORITY, mutual_tls_listener, trusting_the_authority, unnamed};

    /// `url`, as the default instance reads it.
    fn read(url: &str) -> RedisUrl {
        RedisUrl::parse(url, &unnamed()).unwrap_or_else(|refused| panic!("{url}: {refused}"))
    }

    /// A deployment reaching Redis in plaintext says so, whichever URL wrote
    /// it, naming the hosts and never what the URL carries. A URL's hosts
    /// on this machine's loopback say nothing where the link reaches those
    /// alone, which a Sentinel's primary and a Cluster's nodes may not.
    #[test]
    #[expect(
        clippy::result_large_err,
        reason = "figment::Jail fixes the closure's error type"
    )]
    fn a_deployment_reaching_redis_in_plaintext_off_this_machine_is_said() {
        figment::Jail::expect_with(|jail| {
            for (profile, url, endpoint) in [
                (
                    "production",
                    "redis://app:s3cret@cache.internal:6379/2",
                    Some("cache.internal:6379"),
                ),
                ("staging", "redis://cache:6379/", Some("cache:6379")),
                ("", "redis://cache:6379/", Some("cache:6379")),
                ("development", "redis://cache:6379/", None),
                ("test", "redis://cache:6379/", None),
                ("production", "rediss://cache:6379/", None),
                ("production", "redis://127.0.0.1:6379/", None),
                ("production", "redis://localhost:6379/", None),
                (
                    "production",
                    "redis://[2001:db8::1]:6379/",
                    Some("[2001:db8::1]:6379"),
                ),
                ("production", "redis://[::1]:6379/", None),
                ("production", "redis+unix:///run/valkey.sock", None),
                (
                    "production",
                    "redis-sentinel://app:s3cret@a:26379/?node=b:26379&sentinelServiceName=main",
                    Some("a:26379, b:26379"),
                ),
                (
                    "production",
                    "redis-sentinel://127.0.0.1:26379/?sentinelServiceName=main",
                    Some("127.0.0.1:26379"),
                ),
                (
                    "production",
                    "rediss-sentinel://a:26379/?sentinelServiceName=main",
                    None,
                ),
                (
                    "development",
                    "redis-sentinel://a:26379/?sentinelServiceName=main",
                    None,
                ),
                (
                    "production",
                    "redis-cluster://app:s3cret@a:6379/?node=b:6379",
                    Some("a:6379, b:6379"),
                ),
                (
                    "production",
                    "redis-cluster://127.0.0.1:6379/",
                    Some("127.0.0.1:6379"),
                ),
                ("production", "rediss-cluster://a:6379/", None),
                ("test", "redis-cluster://a:6379/", None),
            ] {
                jail.set_env(nest_rs_config::Environment::var_name(), profile);
                let logs = nest_rs_testing::LogCapture::install();
                warn_when_open(&unnamed(), &read(url));
                let lines = logs.find(crate::TARGET, PLAINTEXT_OPEN);
                assert_eq!(
                    lines
                        .iter()
                        .filter_map(|line| line.field("endpoint"))
                        .collect::<Vec<_>>(),
                    endpoint.map(str::to_owned).into_iter().collect::<Vec<_>>(),
                    "{profile:?} {url}",
                );
                for line in lines {
                    assert_eq!(line.level, "warn");
                    assert_eq!(
                        line.field("environment").as_deref(),
                        Some(if profile.is_empty() {
                            "production"
                        } else {
                            profile
                        }),
                    );
                    assert!(!format!("{line:?}").contains("s3cret"), "{line:?}");
                }
            }
            Ok(())
        });
    }

    #[test]
    fn a_tls_clients_material_leaves_a_process_default_provider_installed() {
        assert!(rustls::crypto::CryptoProvider::get_default().is_none());
        material(
            &unnamed(),
            &ClientTls::default(),
            true,
            "redis.internal:6380",
        )
        .expect("nothing set is the system's authorities");
        assert!(
            rustls::crypto::CryptoProvider::get_default().is_some(),
            "a provider is installed for `redis` to build with"
        );
    }

    #[test]
    fn the_material_reaches_the_client_as_set() {
        let Ok(Some(certificates)) = material(&unnamed(), &trusting_the_authority(), true, "x:1")
        else {
            panic!("an authority is material for an encrypted URL");
        };
        assert_eq!(
            certificates.root_cert.as_deref(),
            Some(AUTHORITY.pem().as_bytes())
        );
        assert!(certificates.client_tls.is_none());
        assert!(
            matches!(
                material(&unnamed(), &ClientTls::default(), false, "x:1"),
                Ok(None)
            ),
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
        let Ok(RedisUrl::Standalone(info)) = RedisUrl::parse(&url, &unnamed()) else {
            panic!("{url} is a URL of one server");
        };
        let budget = Duration::from_secs(2);
        let endpoint = addr.to_string();
        let certificates = material(&unnamed(), &trusting_the_authority(), true, &endpoint)
            .expect("the authority is usable");
        let client = crate::connection::open_client(
            &unnamed(),
            info,
            certificates.as_ref(),
            &endpoint,
            budget,
        )
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
            met.is_connection_dropped() && !ClientTls::negotiation_failed(&met),
            "the first command meets a dropped connection, not the refusal: {met}",
        );
        let Err(refused) = heard else {
            panic!("a Redis refusing the client's certificate answers nothing");
        };
        assert!(ClientTls::negotiation_failed(&refused), "{refused}");
        assert!(
            remedy(&unnamed(), &refused).contains("requires a client certificate"),
            "{refused}"
        );
    }

    /// A connection whose every command is answered by `answer`, told how many
    /// commands it was asked before this one.
    struct Answering<F>(u32, F);

    impl<F> ConnectionLike for Answering<F>
    where
        F: FnMut(u32) -> redis::RedisFuture<'static, redis::Value> + Send,
    {
        fn req_packed_command<'a>(
            &'a mut self,
            _: &'a redis::Cmd,
        ) -> redis::RedisFuture<'a, redis::Value> {
            let answer = (self.1)(self.0);
            self.0 += 1;
            answer
        }

        fn req_packed_commands<'a>(
            &'a mut self,
            _: &'a redis::Pipeline,
            _: usize,
            _: usize,
        ) -> redis::RedisFuture<'a, Vec<redis::Value>> {
            unreachable!("`HELLO` is one command")
        }

        fn get_db(&self) -> i64 {
            0
        }
    }

    /// A `HELLO` sent as the connection's driver ended waits on an answer
    /// nothing will give, and the next command meets the closed connection.
    #[tokio::test(start_paused = true)]
    async fn a_hello_stranded_as_its_connection_ended_hears_the_drop_within_its_budget() {
        let budget = Duration::from_secs(8);
        let mut connection = Answering(0, |asked_before| -> redis::RedisFuture<'static, _> {
            if asked_before == 0 {
                Box::pin(std::future::pending())
            } else {
                Box::pin(std::future::ready(Err(std::io::Error::from(
                    std::io::ErrorKind::BrokenPipe,
                )
                .into())))
            }
        });
        let started = tokio::time::Instant::now();

        let Ok(Err(dropped)) = tokio::time::timeout(budget, asked(&mut connection, budget)).await
        else {
            panic!("the stranded `HELLO` waits out the budget nothing else ends");
        };

        assert!(dropped.is_connection_dropped(), "{dropped}");
        assert_eq!(started.elapsed(), budget / 4, "asked again at each quarter");
    }

    /// Asked again, a server that is slow answers all the same, and within the
    /// whole budget its caller gave it.
    #[tokio::test(start_paused = true)]
    async fn a_slow_servers_hello_is_answered_past_each_time_it_is_asked_again() {
        let budget = Duration::from_secs(8);
        let answers_at = tokio::time::Instant::now() + budget - Duration::from_millis(1);
        let mut connection = Answering(0, move |_| -> redis::RedisFuture<'static, _> {
            Box::pin(async move {
                tokio::time::sleep_until(answers_at).await;
                Ok(redis::Value::Map(vec![
                    (
                        redis::Value::SimpleString("mode".to_owned()),
                        redis::Value::SimpleString("standalone".to_owned()),
                    ),
                    (
                        redis::Value::SimpleString("role".to_owned()),
                        redis::Value::SimpleString("master".to_owned()),
                    ),
                ]))
            })
        });

        let hello = tokio::time::timeout(budget, asked(&mut connection, budget))
            .await
            .expect("answered within the budget")
            .expect("a server that answers is heard");

        assert_eq!(hello.serves, crate::RedisTopology::Standalone);
        assert!(hello.primary);
        assert_eq!(connection.0, 4, "asked once per quarter of the budget");
    }
}
