//! Covers `src/module.rs`: the pool an app opens verifies the server's
//! certificate, and logs in to none it cannot verify; the boot's proof refuses
//! what every attempt would meet at once and retries the rest within the
//! connect budget.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nest_rs_core::{App, module};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmModule};
use nest_rs_testing::TestAuthority;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

#[module(imports = [SeaOrmModule::for_root(None)])]
struct PoolModule;

/// The `SSLRequest` a Postgres client opens with: its length, then the code
/// 80877103.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f];

/// A Postgres double that accepts TLS with a certificate an authority of this
/// test issued, which the system does not trust, recording whether a client
/// went on to send its startup message — what it sends, credentials included,
/// once it accepted the server.
async fn untrusted_postgres() -> (String, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let port = listener.local_addr().expect("its address").port();
    let started = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&started);
    let acceptor = TestAuthority::new()
        .server(&["127.0.0.1", "localhost"])
        .acceptor(None);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (acceptor, seen) = (acceptor.clone(), Arc::clone(&seen));
            tokio::spawn(async move {
                let mut request = [0_u8; 8];
                if socket.read_exact(&mut request).await.is_err()
                    || request != SSL_REQUEST
                    || socket.write_all(b"S").await.is_err()
                {
                    return;
                }
                let Ok(mut tls) = acceptor.accept(socket).await else {
                    return;
                };
                let mut startup = [0_u8; 8];
                if tls.read_exact(&mut startup).await.is_ok() {
                    seen.store(true, Ordering::SeqCst);
                }
            });
        }
    });
    (
        format!("postgres://nestrs:s3cret@127.0.0.1:{port}/postgres"),
        started,
    )
}

const RETRYING: &str = "database unreachable — retrying within the connect budget";

/// A certificate no trusted authority signed fails every attempt the same
/// way, so the boot refuses at once — the budget is the test's own, which only
/// a retry could spend — saying where the authority is named.
#[tokio::test]
async fn the_pool_never_logs_in_to_a_server_whose_certificate_no_trusted_authority_signed() {
    let logs = nest_rs_testing::LogCapture::install();
    let (url, started) = untrusted_postgres().await;
    let began = Instant::now();

    let Err(refused) = App::builder()
        .provide(SeaOrmConfig {
            url,
            connect_timeout_secs: Some(60),
            ..Default::default()
        })
        .module::<PoolModule>()
        .build()
        .await
    else {
        panic!("a pool opened on a server it could not verify");
    };

    let rendered = format!("{refused:#}");
    assert!(
        !started.load(Ordering::SeqCst),
        "the pool sent its startup message, credentials included: {rendered}"
    );
    assert!(
        rendered.contains("UnknownIssuer"),
        "refused for the certificate, not another reason: {rendered}"
    );
    assert!(!rendered.contains("s3cret"), "{rendered}");
    assert!(
        began.elapsed() < Duration::from_secs(30),
        "refused at once, not retried: took {:?}",
        began.elapsed()
    );
    assert!(
        rendered.contains("sslrootcert")
            && rendered.contains(&nest_rs_config::var_name("seaorm", "URL")),
        "the refusal says where the authority is named: {rendered}"
    );
    assert!(
        !rendered.contains("  "),
        "one sentence, unpadded: {rendered}"
    );
    logs.expect_none(nest_rs_seaorm::target::ORM, RETRYING);
}

/// A host whose name does not resolve yet — a database started after the app,
/// behind DNS that learns of it then — is tried again within the budget, then
/// refused as unreachable, naming the host and never the URL. `.invalid` never
/// resolves (RFC 6761).
#[tokio::test]
async fn a_host_whose_name_does_not_resolve_is_retried_within_the_budget_then_refused() {
    let logs = nest_rs_testing::LogCapture::install();
    let began = Instant::now();

    let Err(refused) = App::builder()
        .provide(SeaOrmConfig {
            url: "postgres://nestrs:s3cret@nestrs-boot-proof.invalid:5432/app".to_owned(),
            connect_timeout_secs: Some(2),
            ..Default::default()
        })
        .module::<PoolModule>()
        .build()
        .await
    else {
        panic!("a pool opened on a host that does not resolve");
    };

    let rendered = format!("{refused:#}");
    assert!(
        rendered.contains("nestrs-boot-proof.invalid:5432")
            && rendered.contains("could not be reached")
            && rendered.contains(&nest_rs_config::var_name("seaorm", "URL"))
            && rendered.contains(&nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS")),
        "unreachable, naming the host and what to change: {rendered}"
    );
    assert!(!rendered.contains("s3cret"), "{rendered}");
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "within the budget: took {:?}",
        began.elapsed()
    );
    let retried = logs.find(nest_rs_seaorm::target::ORM, RETRYING);
    assert!(retried.len() > 1, "retried: {:#?}", logs.events());
    for event in &retried {
        assert_eq!(event.level, "warn");
        assert_eq!(
            event.field("endpoint").as_deref(),
            Some("nestrs-boot-proof.invalid:5432"),
            "{:?}",
            event.fields
        );
        assert!(
            event.fields.values().all(|value| !value.contains("s3cret")),
            "{:?}",
            event.fields
        );
    }
}

/// A Postgres double answering every startup message with the error a server
/// starting up (`57P03`) or full (`53300`) sends, on a URL without TLS.
async fn postgres_answering(code: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let port = listener.local_addr().expect("its address").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut length = [0_u8; 4];
                if socket.read_exact(&mut length).await.is_err() {
                    return;
                }
                let rest = usize::try_from(u32::from_be_bytes(length))
                    .unwrap_or(0)
                    .saturating_sub(4);
                let mut startup = vec![0_u8; rest];
                if socket.read_exact(&mut startup).await.is_err() {
                    return;
                }
                let mut fields = Vec::new();
                for (kind, value) in [
                    (b'S', "FATAL"),
                    (b'V', "FATAL"),
                    (b'C', code),
                    (b'M', "the database system cannot accept connections yet"),
                ] {
                    fields.push(kind);
                    fields.extend_from_slice(value.as_bytes());
                    fields.push(0);
                }
                fields.push(0);
                let length = u32::try_from(fields.len() + 4).expect("a short message");
                let mut answer = vec![b'E'];
                answer.extend_from_slice(&length.to_be_bytes());
                answer.extend_from_slice(&fields);
                let _ = socket.write_all(&answer).await;
            });
        }
    });
    format!("postgres://nestrs:s3cret@127.0.0.1:{port}/postgres?sslmode=disable")
}

/// Boots a pool on `url` with a 1 s connect budget, which must refuse it as
/// unreachable after retrying, and hands back the refusal as the framework
/// says it and the `error` field of every retry line.
async fn unreachable_after_retrying(url: String) -> (String, Vec<String>) {
    let logs = nest_rs_testing::LogCapture::install();
    let began = Instant::now();
    let Err(refused) = App::builder()
        .provide(SeaOrmConfig {
            url,
            connect_timeout_secs: Some(1),
            ..Default::default()
        })
        .module::<PoolModule>()
        .build()
        .await
    else {
        panic!("a pool opened on a server that let no connection in");
    };
    let rendered = nest_rs_core::error_message(&*refused);
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "within the budget: took {:?}",
        began.elapsed()
    );
    assert!(
        rendered.contains("could not be reached") && !rendered.contains("s3cret"),
        "{rendered}"
    );
    let retried = logs.find(nest_rs_seaorm::target::ORM, RETRYING);
    assert!(
        retried.len() > 1,
        "each attempt said, not one spent in silence: {:#?}",
        logs.events()
    );
    let errors = retried
        .iter()
        .map(|event| {
            assert_eq!(event.level, "warn");
            event.field("error").unwrap_or_default()
        })
        .collect();
    (rendered, errors)
}

/// A Postgres not listening yet — the app started before its database — is
/// dialled again within the budget, each attempt said with what it met, and
/// the refusal carries the last one's cause.
#[tokio::test]
async fn a_refused_connection_is_retried_and_said_per_attempt_then_named_in_the_refusal() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        listener.local_addr().expect("its address").port()
    };
    let (rendered, errors) = unreachable_after_retrying(format!(
        "postgres://nestrs:s3cret@127.0.0.1:{port}/postgres?sslmode=disable"
    ))
    .await;

    assert!(
        rendered.to_lowercase().contains("connection refused"),
        "the refusal's cause is the refused connection: {rendered}"
    );
    for error in errors {
        assert!(
            error.to_lowercase().contains("connection refused"),
            "{error}"
        );
    }
}

/// A server starting up (`57P03`) or full (`53300`) lets the next attempt in
/// once it clears, so each is retried and said, never refused at once.
#[tokio::test]
async fn a_starting_or_full_server_is_retried_and_said_per_attempt() {
    for code in ["57P03", "53300"] {
        let (rendered, errors) = unreachable_after_retrying(postgres_answering(code).await).await;

        assert!(rendered.contains(code), "{code}: {rendered}");
        for error in errors {
            assert!(error.contains(code), "{code}: {error}");
        }
    }
}
