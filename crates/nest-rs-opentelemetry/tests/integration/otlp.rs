//! Covers `src/otlp.rs` — the exporters' HTTP clients, against a collector
//! served over TLS by an in-process double whose certificate a `TestAuthority`
//! issued. Each test initialises OpenTelemetry once, in its own process.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use nest_rs_opentelemetry::{
    FLUSH_TIMEOUT, OpenTelemetry, OpenTelemetryConfig, OpenTelemetryError, TARGET,
};
use nest_rs_testing::{LogCapture, TestAuthority, TestCertificate};
use opentelemetry_otlp::{OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_EXPORTER_OTLP_TIMEOUT};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

/// The authority the collector's certificate chains to; the system's store
/// does not hold it.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// What a collector does with each export it reads.
#[derive(Clone)]
enum Behaviour {
    /// Writes this raw HTTP/1.1 answer, then closes.
    Answer(String),
    /// Holds the connection and never answers.
    Silent,
}

/// The answer of a collector that took the export.
const ACCEPTED: &str = "HTTP/1.1 200 OK\r\ncontent-type: application/x-protobuf\r\n\
                        content-length: 0\r\nconnection: close\r\n\r\n";

/// An OTLP/HTTP collector over TLS on loopback, on a runtime of its own, every
/// export it reads counted.
struct Collector {
    addr: SocketAddr,
    exports: Arc<AtomicUsize>,
}

impl Collector {
    /// A collector taking every export.
    fn serving() -> Self {
        Self::serving_to(Behaviour::Answer(ACCEPTED.to_owned()), None)
    }

    /// A collector doing `behaviour` with every export, requiring a client
    /// certificate `clients` issued when it names an authority.
    fn serving_to(behaviour: Behaviour, clients: Option<&TestAuthority>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        listener.set_nonblocking(true).expect("non-blocking");
        let addr = listener.local_addr().expect("its address");
        let exports = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&exports);
        let acceptor = CERTIFICATE.acceptor(clients);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("the collector's runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("a listener");
                while let Ok((stream, _)) = listener.accept().await {
                    let (acceptor, behaviour, counted) =
                        (acceptor.clone(), behaviour.clone(), Arc::clone(&counted));
                    tokio::spawn(async move {
                        let Ok(mut tls) = acceptor.accept(stream).await else {
                            return;
                        };
                        let mut read = Vec::new();
                        let mut buf = [0_u8; 8192];
                        let body_at = loop {
                            if let Some(at) = read.windows(4).position(|w| w == b"\r\n\r\n") {
                                break at + 4;
                            }
                            match tls.read(&mut buf).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => read.extend_from_slice(&buf[..n]),
                            }
                        };
                        let head = String::from_utf8_lossy(&read[..body_at]).to_ascii_lowercase();
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        while read.len() < body_at + length {
                            match tls.read(&mut buf).await {
                                Ok(0) | Err(_) => return,
                                Ok(n) => read.extend_from_slice(&buf[..n]),
                            }
                        }
                        counted.fetch_add(1, Ordering::SeqCst);
                        match behaviour {
                            Behaviour::Answer(answer) => {
                                let _ = tls.write_all(answer.as_bytes()).await;
                                let _ = tls.shutdown().await;
                            }
                            Behaviour::Silent => std::future::pending::<()>().await,
                        }
                    });
                }
            });
        });
        Self { addr, exports }
    }

    fn endpoint(&self) -> String {
        format!("https://127.0.0.1:{}", self.addr.port())
    }

    fn exports(&self) -> usize {
        self.exports.load(Ordering::SeqCst)
    }
}

/// `config` initialised with `vars` — the OpenTelemetry specification's own,
/// unprefixed — set over the process's environment while the exporters are
/// built.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn init_under(
    vars: &[(&str, &str)],
    config: OpenTelemetryConfig,
) -> Result<OpenTelemetry, OpenTelemetryError> {
    let mut init = None;
    figment::Jail::expect_with(|jail| {
        for (key, value) in vars {
            jail.set_env(key, value);
        }
        init = Some(OpenTelemetry::init_with(config));
        Ok(())
    });
    init.expect("the jail ran")
}

/// A file `name` holding `contents` in `scratch`, which the test holds: the
/// specification's certificate variables name paths.
fn written(scratch: &TempDir, name: &str, contents: &str) -> String {
    let path = scratch.path().join(name);
    std::fs::write(&path, contents).expect("a scratch file");
    path.to_string_lossy().into_owned()
}

/// Emit a span and a line, then flush them on the guard's drop.
fn export_through(otel: OpenTelemetry) {
    tracing::info_span!("probe").in_scope(|| tracing::info!(target: "probe", "probe"));
    drop(otel);
}

/// `OpenTelemetry::init_with` exporting to `collector`, trusting [`AUTHORITY`]
/// through `OTEL_EXPORTER_OTLP_CERTIFICATE`, with `vars` over it.
fn trusting(collector: &Collector, vars: &[(&str, &str)]) -> OpenTelemetry {
    let endpoint = collector.endpoint();
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let authority = written(&scratch, "authority.pem", AUTHORITY.pem());
    let mut set = vec![
        (OTEL_EXPORTER_OTLP_ENDPOINT, endpoint.as_str()),
        ("OTEL_EXPORTER_OTLP_CERTIFICATE", authority.as_str()),
    ];
    set.extend_from_slice(vars);
    init_under(&set, OpenTelemetryConfig::default()).expect("the exporters build")
}

#[test]
fn an_export_reaches_a_collector_behind_a_private_authority_named_by_the_specification() {
    let collector = Collector::serving();

    export_through(trusting(&collector, &[]));

    assert!(collector.exports() > 0, "the flush reached the collector");
}

/// The endpoint pinned in code exports as the variable's does, under the
/// authority a pinned `ClientTls` names.
#[test]
fn an_export_reaches_a_collector_named_in_code() {
    let collector = Collector::serving();
    let config = OpenTelemetryConfig {
        otlp_endpoint: Some(collector.endpoint()),
        tls: AUTHORITY.client_tls(),
        ..OpenTelemetryConfig::default()
    };

    export_through(OpenTelemetry::init_with(config).expect("the exporters build"));

    assert!(collector.exports() > 0, "the flush reached the collector");
}

#[test]
fn an_export_to_a_collector_no_trusted_authority_signed_never_lands() {
    let collector = Collector::serving();
    let otel = init_under(
        &[(OTEL_EXPORTER_OTLP_ENDPOINT, &collector.endpoint())],
        OpenTelemetryConfig::default(),
    )
    .expect("the exporters build");

    export_through(otel);

    assert_eq!(
        collector.exports(),
        0,
        "no export crossed a handshake the client refused"
    );
}

/// An `http://` endpoint declares the export in plaintext: TLS material beside
/// it would go unused, and is refused naming both settings.
#[test]
fn tls_material_beside_a_plaintext_endpoint_is_refused_naming_both() {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let authority = written(&scratch, "plaintext-authority.pem", AUTHORITY.pem());
    let Err(refused) = init_under(
        &[
            (OTEL_EXPORTER_OTLP_ENDPOINT, "http://127.0.0.1:4318"),
            ("OTEL_EXPORTER_OTLP_CERTIFICATE", &authority),
        ],
        OpenTelemetryConfig::default(),
    ) else {
        panic!("TLS material beside a plaintext endpoint must not boot");
    };
    assert!(
        matches!(refused, OpenTelemetryError::Config(_)),
        "{refused}"
    );
    let sentence = refused.to_string();
    assert!(
        sentence.contains("OTEL_EXPORTER_OTLP_CERTIFICATE")
            && sentence.contains(OTEL_EXPORTER_OTLP_ENDPOINT),
        "{sentence}"
    );
}

/// A collector requiring a client certificate is handed the one the deployment
/// names under `OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE` and `_CLIENT_KEY`.
#[test]
fn a_collector_requiring_a_client_certificate_is_handed_the_configured_one() {
    let clients = TestAuthority::new();
    let issued = clients.client("nestrs-test-client");
    let collector = Collector::serving_to(Behaviour::Answer(ACCEPTED.to_owned()), Some(&clients));
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let cert = written(&scratch, "client.pem", &issued.cert);
    let key = written(&scratch, "client-key.pem", &issued.key);

    export_through(trusting(
        &collector,
        &[
            ("OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE", &cert),
            ("OTEL_EXPORTER_OTLP_CLIENT_KEY", &key),
        ],
    ));

    assert!(collector.exports() > 0, "the flush reached the collector");
}

/// Without a client certificate, a collector requiring one takes no export:
/// what makes the test above a proof.
#[test]
fn a_collector_requiring_a_client_certificate_takes_nothing_without_one() {
    let clients = TestAuthority::new();
    let collector = Collector::serving_to(Behaviour::Answer(ACCEPTED.to_owned()), Some(&clients));

    export_through(trusting(&collector, &[]));

    assert_eq!(
        collector.exports(),
        0,
        "no export crossed the refused handshake"
    );
}

/// A redirect is an answer, never followed: a collector's `307` could send the
/// telemetry, and the collector's credential, anywhere.
#[test]
fn a_redirect_from_the_collector_is_never_followed() {
    let elsewhere = Collector::serving();
    let redirecting = Collector::serving_to(
        Behaviour::Answer(format!(
            "HTTP/1.1 307 Temporary Redirect\r\nlocation: {}/v1/traces\r\n\
             content-length: 0\r\nconnection: close\r\n\r\n",
            elsewhere.endpoint()
        )),
        None,
    );

    export_through(trusting(&redirecting, &[]));

    assert!(redirecting.exports() > 0, "the flush reached the collector");
    assert_eq!(elsewhere.exports(), 0, "the redirect was not followed");
}

/// An export is cut at the SDK's export timeout, which the deployment sets
/// under `OTEL_EXPORTER_OTLP_TIMEOUT`: a collector that never answers holds the
/// final flush that long, never the flush's own bound.
#[test]
fn an_export_is_cut_at_the_timeout_the_sdk_reads() {
    let collector = Collector::serving_to(Behaviour::Silent, None);
    let otel = trusting(&collector, &[(OTEL_EXPORTER_OTLP_TIMEOUT, "200")]);

    let started = Instant::now();
    export_through(otel);
    let took = started.elapsed();

    assert!(collector.exports() > 0, "an export was in flight");
    assert!(
        took < FLUSH_TIMEOUT / 2,
        "the exports were cut at 200 ms, not held to the flush's {FLUSH_TIMEOUT:?}: {took:?}"
    );
}

/// An export timeout that is not a whole number of milliseconds is passed over,
/// as the SDK passes it over, and said once at `warn` by its variable.
#[test]
fn an_export_timeout_that_is_not_milliseconds_is_ignored_and_said() {
    let collector = Collector::serving();
    let logs = LogCapture::install();

    let otel = trusting(&collector, &[(OTEL_EXPORTER_OTLP_TIMEOUT, "10s")]);

    let event = logs.expect_one(
        TARGET,
        "an OpenTelemetry variable is not a whole number of milliseconds; ignored, as the SDK \
         ignores it",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("variable").as_deref(),
        Some(OTEL_EXPORTER_OTLP_TIMEOUT)
    );
    drop(otel);
}
