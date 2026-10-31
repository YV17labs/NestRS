//! Covers `src/otlp.rs` — the exporters' HTTP clients, against a collector
//! served over TLS by an in-process double whose certificate a `TestAuthority`
//! issued. Each test initialises OpenTelemetry once, in its own process.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use nest_rs_opentelemetry::{FLUSH_TIMEOUT, OpenTelemetry, OpenTelemetryConfig, TARGET};
use nest_rs_testing::{LogCapture, TestAuthority, TestCertificate};
use opentelemetry_otlp::{OTEL_EXPORTER_OTLP_ENDPOINT, OTEL_EXPORTER_OTLP_TIMEOUT};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::{init_under, refused_under};

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

/// One export a collector read: where it was posted, and how its body was
/// encoded.
#[derive(Clone)]
struct Received {
    path: String,
    /// Its `content-encoding`, lowercased.
    encoding: Option<String>,
    body: Vec<u8>,
}

/// Its path, encoding and size, never its bytes.
impl std::fmt::Debug for Received {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {:?} ({} bytes)",
            self.path,
            self.encoding,
            self.body.len()
        )
    }
}

/// An OTLP/HTTP collector over TLS on loopback, on a runtime of its own, every
/// export it reads kept.
struct Collector {
    addr: SocketAddr,
    exports: Arc<Mutex<Vec<Received>>>,
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
        let exports = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&exports);
        let acceptor = CERTIFICATE.acceptor(clients);
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("the collector's runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("a listener");
                while let Ok((stream, _)) = listener.accept().await {
                    let (acceptor, behaviour, kept) =
                        (acceptor.clone(), behaviour.clone(), Arc::clone(&kept));
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
                        let path = head
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or_default();
                        let encoding = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-encoding:"))
                            .map(|value| value.trim().to_owned());
                        kept.lock().expect("the exports").push(Received {
                            path: path.to_owned(),
                            encoding,
                            body: read[body_at..].to_vec(),
                        });
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
        self.exports.lock().expect("the exports").len()
    }

    /// How many exports were posted to `path`.
    fn exports_to(&self, path: &str) -> usize {
        self.received()
            .iter()
            .filter(|received| received.path == path)
            .count()
    }

    /// Every export read so far.
    fn received(&self) -> Vec<Received> {
        self.exports.lock().expect("the exports").clone()
    }
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

    export_through(init_under(&[], config).expect("the exporters build"));

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
    let sentence = refused_under(
        &[
            (OTEL_EXPORTER_OTLP_ENDPOINT, "http://127.0.0.1:4318"),
            ("OTEL_EXPORTER_OTLP_CERTIFICATE", &authority),
        ],
        OpenTelemetryConfig::default(),
    );
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

/// A signal its selector switches off ships nothing to the endpoint every
/// signal shares, while the others export there.
#[test]
fn a_signal_its_selector_switches_off_exports_nothing_beside_one_that_exports() {
    let collector = Collector::serving();

    export_through(trusting(&collector, &[("OTEL_TRACES_EXPORTER", "none")]));

    assert!(collector.exports_to("/v1/logs") > 0, "the logs exported");
    assert_eq!(
        collector.exports_to("/v1/traces"),
        0,
        "OTEL_TRACES_EXPORTER=none ships no span"
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
        "an OpenTelemetry variable does not hold what it must; passed over, as the \
         specification orders",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("variable").as_deref(),
        Some(OTEL_EXPORTER_OTLP_TIMEOUT)
    );
    assert_eq!(
        event.field("expected").as_deref(),
        Some("a whole number of milliseconds")
    );
    assert!(!event.quotes("10s"), "{event:?}");
    drop(otel);
}

/// A deployment carried over from `<PREFIX>_OPENTELEMETRY__*` exports nothing
/// through it, and is told so once per variable.
#[test]
fn a_variable_of_the_retired_namespace_is_said_at_warn() {
    let logs = LogCapture::install();
    let retired = nest_rs_config::var_name("opentelemetry", "OTLP_ENDPOINT");

    let otel = init_under(
        &[(retired.as_str(), "http://collector:4318")],
        OpenTelemetryConfig::default(),
    )
    .expect("a retired variable fails no boot");

    let event = logs.expect_one(
        TARGET,
        "variable read by nothing: OpenTelemetry reads the OpenTelemetry specification's OTEL_* \
         variables",
    );
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("variable").as_deref(), Some(retired.as_str()));
    assert!(!event.quotes("collector:4318"), "{event:?}");
    drop(otel);
}

/// The specification's `gzip` is honoured: every export reaches the collector
/// compressed, said by its `content-encoding` and its body's gzip header.
#[test]
fn otel_exporter_otlp_compression_gzip_compresses_every_export() {
    let collector = Collector::serving();

    export_through(trusting(
        &collector,
        &[("OTEL_EXPORTER_OTLP_COMPRESSION", "gzip")],
    ));

    let received = collector.received();
    assert!(!received.is_empty(), "the flush reached the collector");
    for export in received {
        assert_eq!(export.encoding.as_deref(), Some("gzip"), "{export:?}");
        assert!(export.body.starts_with(&[0x1f, 0x8b]), "{export:?}");
    }
}

/// A signal's own `none` keeps its exports plain under a general `gzip`, as the
/// specification orders the two.
#[test]
fn a_signal_s_own_compression_wins_over_the_general_one() {
    let collector = Collector::serving();

    export_through(trusting(
        &collector,
        &[
            ("OTEL_EXPORTER_OTLP_COMPRESSION", "GZIP"),
            ("OTEL_EXPORTER_OTLP_LOGS_COMPRESSION", "none"),
        ],
    ));

    let received = collector.received();
    let logs: Vec<&Received> = received.iter().filter(|r| r.path == "/v1/logs").collect();
    assert!(!logs.is_empty(), "the logs exported");
    assert!(logs.iter().all(|r| r.encoding.is_none()), "{logs:?}");
    let traces: Vec<&Received> = received.iter().filter(|r| r.path == "/v1/traces").collect();
    assert!(!traces.is_empty(), "the traces exported");
    assert!(
        traces.iter().all(|r| r.encoding.as_deref() == Some("gzip")),
        "{traces:?}"
    );
}

/// A compression this exporter does not build is refused naming its variable,
/// never passed over while the deployment believes its exports compressed.
#[test]
fn a_compression_this_exporter_does_not_build_is_refused_naming_its_variable() {
    let said = refused_under(
        &[
            (OTEL_EXPORTER_OTLP_ENDPOINT, "http://127.0.0.1:4318"),
            ("OTEL_EXPORTER_OTLP_TRACES_COMPRESSION", "zstd"),
        ],
        OpenTelemetryConfig::default(),
    );
    assert!(
        said.contains("OTEL_EXPORTER_OTLP_TRACES_COMPRESSION") && said.contains("gzip"),
        "{said}"
    );
    assert!(!said.contains("zstd"), "{said}");
}

/// A collector's credential never crosses the network in clear: a header set
/// beside an `http://` endpoint on another host refuses the boot, naming both
/// variables and quoting neither value.
#[test]
fn a_header_beside_a_plaintext_endpoint_on_another_host_is_refused_naming_both() {
    let said = refused_under(
        &[
            (
                OTEL_EXPORTER_OTLP_ENDPOINT,
                "http://collector.internal:4318",
            ),
            ("OTEL_EXPORTER_OTLP_HEADERS", "authorization=Bearer s3cr3t"),
        ],
        OpenTelemetryConfig::default(),
    );
    assert!(
        said.contains("OTEL_EXPORTER_OTLP_HEADERS") && said.contains(OTEL_EXPORTER_OTLP_ENDPOINT),
        "{said}"
    );
    assert!(
        !said.contains("s3cr3t") && !said.contains("Bearer"),
        "{said}"
    );
}

/// On this host's loopback, where the credential never leaves the machine — a
/// collector beside the app — a header over plaintext boots.
#[test]
fn a_header_beside_a_plaintext_endpoint_on_loopback_boots() {
    let otel = init_under(
        &[
            (OTEL_EXPORTER_OTLP_ENDPOINT, "http://localhost:4318"),
            ("OTEL_TRACES_EXPORTER", "none"),
            ("OTEL_METRICS_EXPORTER", "none"),
            (
                "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
                "authorization=Bearer s3cr3t",
            ),
            (
                "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
                "http://127.0.0.1:9/v1/logs",
            ),
        ],
        OpenTelemetryConfig::default(),
    )
    .expect("a header to a collector on loopback");
    drop(otel);
}

/// A proxy that counts the connections it is offered and serves none.
fn proxy() -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("its address"));
    let offered = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&offered);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    (url, offered)
}

/// A proxy the environment names reaches its own host's loopback, never this
/// one's, and would carry a header sent there off the machine: a collector on
/// loopback is dialled directly, the export landing and the proxy offered
/// nothing.
#[test]
fn an_export_to_a_collector_on_loopback_never_goes_through_a_proxy() {
    let collector = Collector::serving();
    let (proxy, offered) = proxy();

    export_through(trusting(
        &collector,
        &[
            ("HTTP_PROXY", proxy.as_str()),
            ("HTTPS_PROXY", proxy.as_str()),
            ("ALL_PROXY", proxy.as_str()),
            ("NO_PROXY", ""),
            ("no_proxy", ""),
            ("OTEL_EXPORTER_OTLP_HEADERS", "authorization=Bearer s3cr3t"),
        ],
    ));

    assert!(collector.exports() > 0, "the flush reached the collector");
    assert_eq!(
        offered.load(Ordering::SeqCst),
        0,
        "the proxy was offered an export"
    );
}

/// `OTEL_METRIC_EXPORT_TIMEOUT` bounds a metric export beside the exporter's
/// own timeout, the earlier of the two cutting it: a collector that never
/// answers holds the final flush that long, never the flush's own bound.
#[test]
fn otel_metric_export_timeout_cuts_a_metric_export() {
    let collector = Collector::serving_to(Behaviour::Silent, None);
    let otel = trusting(
        &collector,
        &[
            ("OTEL_TRACES_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
            ("OTEL_METRIC_EXPORT_TIMEOUT", "200"),
        ],
    );
    opentelemetry::global::meter_provider()
        .meter("probe")
        .u64_counter("probe")
        .build()
        .add(1, &[]);

    let started = Instant::now();
    drop(otel);
    let took = started.elapsed();

    assert!(
        collector.exports_to("/v1/metrics") > 0,
        "a metric export was in flight"
    );
    assert!(
        took < FLUSH_TIMEOUT / 2,
        "the metric export was cut at 200 ms, not held to the flush's {FLUSH_TIMEOUT:?}: {took:?}"
    );
}

/// A sampler the SDK does not build, or a ratio that does not read as one, is
/// refused naming its variable: the SDK would sample every trace in its place,
/// over the ratio the app pinned.
#[test]
fn a_sampler_or_a_ratio_the_sdk_would_pass_over_is_refused_naming_its_variable() {
    let said = refused_under(
        &[
            ("OTEL_TRACES_SAMPLER", "parentbased_traceidratio"),
            ("OTEL_TRACES_SAMPLER_ARG", "10%"),
        ],
        OpenTelemetryConfig {
            trace_sample_ratio: 0.1,
            ..OpenTelemetryConfig::default()
        },
    );
    assert!(
        said.contains("OTEL_TRACES_SAMPLER_ARG") && said.contains("between 0 and 1"),
        "{said}"
    );
    assert!(!said.contains("10%"), "{said}");
}

/// A metric temporality the SDK does not know is refused naming its variable,
/// where the SDK would export cumulative sums the backend did not ask for.
#[test]
fn a_metric_temporality_the_sdk_does_not_know_is_refused_naming_its_variable() {
    let said = refused_under(
        &[
            (OTEL_EXPORTER_OTLP_ENDPOINT, "http://127.0.0.1:4318"),
            (
                "OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE",
                "deltas",
            ),
        ],
        OpenTelemetryConfig::default(),
    );
    assert!(
        said.contains("OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE")
            && said.contains("cumulative"),
        "{said}"
    );
    assert!(!said.contains("deltas"), "{said}");
}
