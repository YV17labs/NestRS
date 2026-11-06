//! The shared connection, without a Redis: a scripted server that stays busy,
//! refuses a `SELECT` or is demoted under its clients, and the budget's place
//! below the ports' nets.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use nest_rs_redis::{RedisConfig, RedisConnection, RedisError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;

use crate::harness::connection::{
    NOT_READY, ScriptedRedis, answer, answer_each_command, config, database_refused_at_once,
};
use crate::{HELLO, is_hello};

/// How long a connect may run before the test calls it hung: a guard alone,
/// far past every budget a test here sets, so a loaded machine never reaches
/// it.
const HUNG: Duration = Duration::from_secs(60);

/// A Redis that stays not ready spends the budget and fails as one that
/// answered — its last answer as the source, the budget to widen — never as an
/// unreachable one sent to check its URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redis_that_stays_busy_fails_at_the_budget_naming_its_answer() {
    let proxy = ScriptedRedis::start(None, None).await;
    proxy.answer_with(Some(NOT_READY[0]));
    let budget = Duration::from_millis(1500);
    let started = Instant::now();
    let Ok(Err(error)) = tokio::time::timeout(
        HUNG,
        RedisConnection::connect(&RedisConfig {
            url: proxy.url().into(),
            connect_timeout: budget,
            tls: crate::harness::tls::trusting_the_test_authority(),
            ..RedisConfig::default()
        }),
    )
    .await
    else {
        panic!("a Redis that never serves must not connect, and must not hang")
    };
    let took = started.elapsed();

    assert!(took >= budget, "not before the budget, took {took:?}");
    assert!(
        matches!(error, RedisError::Unready { budget: spent, .. } if spent == budget),
        "the budget ended the attempts: {error}"
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains(&nest_rs_config::var_name("redis", "CONNECT_TIMEOUT_SECS"))
            && !rendered.contains(&nest_rs_config::var_name("redis", "URL")),
        "the budget is what to widen, not the URL to check: {rendered}"
    );
    assert!(
        answer(&error).contains("busy running a script"),
        "the source is Redis's last answer: {}",
        answer(&error)
    );
}

/// config-1r2: a server in cluster mode answers a `SELECT` with an `ERR` the
/// client reports without its code, so it was retried for the whole budget. It
/// fails at once, naming the index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_serves_database_zero_alone_fails_the_boot_at_once_naming_the_index() {
    let proxy = ScriptedRedis::start(None, None).await;
    proxy.answer_with(Some("ERR SELECT is not allowed in cluster mode"));
    let error = database_refused_at_once(
        RedisConfig {
            tls: crate::harness::tls::trusting_the_test_authority(),
            ..config(format!("{}2", proxy.url()))
        },
        2,
        "a server in cluster mode",
    )
    .await;
    assert!(
        answer(&error).contains("not allowed in cluster mode"),
        "the source says what Redis answered: {}",
        answer(&error),
    );
}

/// What marks the URL's password, and whatever else it holds, in anything that
/// prints.
const PASSWORD: &str = "zzqx-password";

/// Connect on `config`, which must be refused, and hold everything the refusal
/// prints and every line it files to quoting no [`PASSWORD`].
async fn refused_quoting_no_password(case: &str, config: RedisConfig) {
    let logs = nest_rs_testing::LogCapture::install();
    let Ok(Err(error)) = tokio::time::timeout(HUNG, RedisConnection::connect(&config)).await else {
        panic!("{case}: must not connect, and must not hang");
    };
    let printed = nest_rs_testing::printed(&error);
    assert!(!printed.contains(PASSWORD), "{case}: {printed}");
    for event in logs.events() {
        assert!(!event.quotes(PASSWORD), "{case}: {event:#?}");
    }
}

/// What a `main` returning the boot's refusal prints is every link as it is, the
/// client's own among them: no URL holding a password or a mistyped setting has
/// it quoted, on any topology, before anything is dialled or after.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_refusal_of_a_url_prints_what_the_url_holds() {
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        listener.local_addr().expect("its address")
    };
    let budget = Duration::from_millis(300);
    let urls = [
        (
            "nothing listens",
            format!("rediss://app:{PASSWORD}@{closed}/0"),
        ),
        (
            "no sentinel listens",
            format!(
                "rediss-sentinel://app:{PASSWORD}@{closed}/0?sentinelServiceName=main\
                 &sentinelUsername=watch&sentinelPassword={PASSWORD}"
            ),
        ),
        (
            "no seed listens",
            format!("rediss-cluster://app:{PASSWORD}@{closed}/0"),
        ),
        (
            "no such host",
            format!("rediss://app:{PASSWORD}@no-such-host.invalid:6379/0"),
        ),
        (
            "a URL that does not parse",
            format!("rediss://app:{PASSWORD}@[::bad-host/"),
        ),
        (
            "a URL naming several hosts that does not parse",
            format!("rediss-cluster://app:{PASSWORD}@[::bad-host/"),
        ),
        (
            "another scheme",
            format!("https://app:{PASSWORD}@{closed}/0"),
        ),
        (
            "a database that is no number",
            format!("rediss://app:{PASSWORD}@{closed}/{PASSWORD}"),
        ),
        (
            "a cluster database that is no number",
            format!("rediss-cluster://app:{PASSWORD}@{closed}/{PASSWORD}"),
        ),
        (
            "a fragment",
            format!("rediss://app:{PASSWORD}@{closed}/0#{PASSWORD}"),
        ),
        (
            "a fragment on several hosts",
            format!("rediss-cluster://app:{PASSWORD}@{closed}/0#{PASSWORD}"),
        ),
        (
            "a skipped verification",
            format!("rediss://app:{PASSWORD}@{closed}/0#insecure"),
        ),
        (
            "RESP3",
            format!("rediss://app:{PASSWORD}@{closed}/0?protocol=resp3"),
        ),
        (
            "a protocol nobody speaks",
            format!("rediss://app:{PASSWORD}@{closed}/0?protocol={PASSWORD}"),
        ),
        (
            "a parameter several hosts take",
            format!("rediss://app:{PASSWORD}@{closed}/0?sentinelPassword={PASSWORD}"),
        ),
        (
            "a parameter nothing takes",
            format!("rediss-cluster://app:{PASSWORD}@{closed}/0?{PASSWORD}={PASSWORD}"),
        ),
        (
            "a node that is no host",
            format!("rediss-cluster://app:{PASSWORD}@{closed}/0?node=x:{PASSWORD}"),
        ),
        (
            "a host no certificate can name",
            format!("rediss://app:{PASSWORD}@under_score.invalid:6379/0"),
        ),
        (
            "a primary with no name",
            format!("rediss-sentinel://app:{PASSWORD}@{closed}/0"),
        ),
    ];
    for (case, url) in urls {
        refused_quoting_no_password(
            case,
            RedisConfig {
                url: url.into(),
                connect_timeout: budget,
                ..RedisConfig::default()
            },
        )
        .await;
    }
}

/// A server's own answer is what a refusal carries: none of the ones that end a
/// boot repeats the password the client authenticated with, even the words a
/// server not knowing `AUTH` answers with, which quote its arguments.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_refusal_a_server_answers_prints_the_password_it_was_sent() {
    const ANSWERS: [&str; 5] = [
        "WRONGPASS invalid username-password pair or user is disabled.",
        "NOAUTH Authentication required.",
        "NOPERM this user has no permissions to run the 'select' command",
        "LOADING Redis is loading the dataset in memory",
        "ERR unknown command 'AUTH', with args beginning with: 'app' 'zzqx-password' ",
    ];
    for answer in ANSWERS {
        let proxy = ScriptedRedis::start(None, None).await;
        proxy.answer_with(Some(answer));
        let authority = proxy.url().replacen("rediss://", "", 1);
        refused_quoting_no_password(
            answer,
            RedisConfig {
                url: format!("rediss://app:{PASSWORD}@{authority}0").into(),
                connect_timeout: Duration::from_millis(300),
                tls: crate::harness::tls::trusting_the_test_authority(),
                ..RedisConfig::default()
            },
        )
        .await;
    }
}

/// A private key holding the password's mark, pasted inline: nothing refusing it
/// quotes the material.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_refusal_of_tls_material_prints_the_material() {
    use nest_rs_config::{ClientTls, Material, TlsIdentity};

    let garbled = format!("-----BEGIN PRIVATE KEY-----\n{PASSWORD}\n-----END PRIVATE KEY-----\n");
    for tls in [
        ClientTls::new(Some(Material::inline(garbled.clone())), None),
        ClientTls::new(
            None,
            Some(TlsIdentity::new(
                Material::inline(garbled.clone()),
                Material::inline(garbled.clone()),
            )),
        ),
    ] {
        refused_quoting_no_password(
            "garbled material",
            RedisConfig {
                url: "rediss://127.0.0.1:9/".to_owned().into(),
                connect_timeout: Duration::from_millis(300),
                tls,
                ..RedisConfig::default()
            },
        )
        .await;
    }
}

/// The default budget sits below the net of every port a binding serves — the
/// queue's, the throttler guard's and the scheduler's — read from the constants
/// that set them rather than retyped. A deployment that raises it is held under
/// each net by the boot, not by room left here: the queue's and the throttler's
/// sit under the HTTP edge's request timeout, which bounds them from above.
#[test]
fn the_connection_budget_answers_before_every_ports_net() {
    let budget = RedisConfig::default().connect_timeout;
    for (net, what) in [
        (nest_rs_queue::BACKEND_TIMEOUT, "the queue port's net"),
        (nest_rs_throttler::HIT_TIMEOUT, "the throttler guard's net"),
        (nest_rs_schedule::LOCK_TIMEOUT, "the scheduler's net"),
    ] {
        assert!(
            budget < net,
            "the connection budget ({budget:?}) must answer before {what} ({net:?})"
        );
    }
}

/// A primary demoted under a live connection — a failover behind a name that
/// now reaches the new primary — answers every write `READONLY`. The connection
/// is reopened at the first, so the writes after it reach the new primary
/// rather than failing until the process restarts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_to_a_demoted_primary_is_reopened() {
    let logs = nest_rs_testing::LogCapture::install_global();
    let server = DemotedRedis::start().await;
    let mut conn = RedisConnection::connect(&RedisConfig {
        url: server.url().into(),
        tls: crate::harness::tls::trusting_the_test_authority(),
        ..RedisConfig::default()
    })
    .await
    .expect("the primary serves the boot");
    server.demote();
    let write = || redis::cmd("SET").arg("nestrs-demoted").arg("1").clone();
    let refused = write()
        .query_async::<()>(&mut conn)
        .await
        .expect_err("the demoted primary refuses the write");
    assert_eq!(refused.code(), Some("READONLY"), "{refused}");

    // Left on the demoted primary, every write fails until the process
    // restarts: the guard only turns that into a failure rather than a hang.
    nest_rs_testing::wait_for(HUNG, || {
        let (write, mut conn) = (write(), conn.clone());
        async move { write.query_async::<()>(&mut conn).await.is_ok() }
    })
    .await;
    let said = logs.expect_one(
        nest_rs_redis::TARGET,
        "redis connection opened again: the server it reached answered as a read-only replica",
    );
    assert_eq!(said.level, "warn");
}

/// A primary demoted under its clients: until [`demote`](Self::demote), every
/// connection is answered `+PONG`; after it, the connections it had accepted
/// answer every command `READONLY`, as a demoted primary does, and the ones it
/// accepts next answer `+PONG` again — the name now reaching the new primary.
pub(crate) struct DemotedRedis {
    addr: SocketAddr,
    accepted: Arc<AtomicUsize>,
    demoted_below: Arc<AtomicUsize>,
}

impl DemotedRedis {
    pub(crate) async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the server");
        let addr = listener.local_addr().expect("the server's address");
        let accepted = Arc::new(AtomicUsize::new(0));
        let demoted_below = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&accepted);
        let demoting = Arc::clone(&demoted_below);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let id = counting.fetch_add(1, Ordering::SeqCst);
                let demoting = Arc::clone(&demoting);
                tokio::spawn(async move {
                    if let Some(client) = crate::harness::tls::accept(client).await {
                        answer_as_its_role(client, id, &demoting).await;
                    }
                });
            }
        });
        Self {
            addr,
            accepted,
            demoted_below,
        }
    }

    pub(crate) fn url(&self) -> String {
        format!("rediss://{}/", self.addr)
    }

    /// The config to seed for an instance on this server: the suite's own
    /// `<PREFIX>_REDIS__URL` would outrank a pin.
    pub(crate) fn config(&self) -> RedisConfig {
        RedisConfig {
            url: self.url().into(),
            connect_timeout: Duration::from_secs(2),
            tls: crate::harness::tls::trusting_the_test_authority(),
        }
    }

    /// Demote every connection accepted so far.
    pub(crate) fn demote(&self) {
        self.demoted_below
            .store(self.accepted.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

/// Answer each command connection `id` sends: its `HELLO` as a primary's,
/// then `READONLY` once it is below the demotion, `+PONG` otherwise.
async fn answer_as_its_role(
    client: impl AsyncRead + AsyncWrite + Unpin,
    id: usize,
    demoted_below: &AtomicUsize,
) {
    answer_each_command(client, |command| {
        let reply: &[u8] = if is_hello(command) {
            HELLO
        } else if id < demoted_below.load(Ordering::SeqCst) {
            b"-READONLY You can't write against a read only replica.\r\n"
        } else {
            b"+PONG\r\n"
        };
        Some(reply)
    })
    .await;
}
