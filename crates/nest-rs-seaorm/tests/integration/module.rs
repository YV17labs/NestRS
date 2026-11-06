//! Covers `src/module.rs`: the pool an app opens verifies the server's
//! certificate, and logs in to none it cannot verify; the boot's proof refuses
//! what every attempt would meet at once and retries the rest within the
//! connect budget; nothing it says, by any reader, quotes the URL's password.
//! Covers `src/config.rs` where libpq's own TLS variables meet that pool.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nest_rs_core::{App, module};
use nest_rs_seaorm::{SeaOrmConfig, SeaOrmModule};
use nest_rs_testing::TestAuthority;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

#[module(imports = [SeaOrmModule])]
struct PoolModule;

/// The `SSLRequest` a Postgres client opens with: its length, then the code
/// 80877103.
const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 0x04, 0xd2, 0x16, 0x2f];

/// A Postgres double that accepts TLS with a certificate `authority` issued,
/// asking one of `clients` for a client certificate when it names them,
/// recording whether a client went on to send its startup message — what it
/// sends, credentials included, once it accepted the server — and refusing that
/// login at once.
async fn postgres_signed_by(
    authority: &TestAuthority,
    clients: Option<&TestAuthority>,
) -> (String, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let port = listener.local_addr().expect("its address").port();
    let started = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&started);
    let acceptor = authority
        .server(&["127.0.0.1", "localhost"])
        .acceptor(clients);
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
                    let _ = tls
                        .write_all(&error_response(LOGIN_REFUSED, "the double refuses"))
                        .await;
                    let _ = tls.shutdown().await;
                }
            });
        }
    });
    (
        format!("postgres://nestrs:s3cret@127.0.0.1:{port}/postgres"),
        started,
    )
}

/// One Postgres message: its tag, then its length, which counts itself.
fn postgres_message(tag: u8, body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(body.len() + 4).expect("a short message");
    let mut message = vec![tag];
    message.extend_from_slice(&length.to_be_bytes());
    message.extend_from_slice(body);
    message
}

/// The `ErrorResponse` a Postgres answers with: `code` in the words `message`
/// gives, each field a kind and a value.
fn error_response(code: &str, message: &str) -> Vec<u8> {
    let mut fields = Vec::new();
    for (kind, value) in [
        (b'S', "FATAL"),
        (b'V', "FATAL"),
        (b'C', code),
        (b'M', message),
    ] {
        fields.push(kind);
        fields.extend_from_slice(value.as_bytes());
        fields.push(0);
    }
    fields.push(0);
    postgres_message(b'E', &fields)
}

/// The SQLSTATE of a login Postgres refuses, which the boot refuses at once.
const LOGIN_REFUSED: &str = "28P01";

const RETRYING: &str = "database unreachable — retrying within the connect budget";

/// How long a boot may run before the test calls it hung: a guard alone, far
/// past every budget a test here sets, so a loaded machine never reaches it.
const HUNG: Duration = Duration::from_secs(60);

/// A certificate no trusted authority signed fails every attempt the same
/// way, so the boot refuses at once — the budget is the test's own, which only
/// a retry could spend — saying where the authority is named.
#[tokio::test]
async fn the_pool_never_logs_in_to_a_server_whose_certificate_no_trusted_authority_signed() {
    let logs = nest_rs_testing::LogCapture::install();
    let (url, started) = postgres_signed_by(&TestAuthority::new(), None).await;
    // A refusal retried ends with the budget, so one said before it was said
    // at once, however loaded the machine.
    let only_a_retry_spends = Duration::from_secs(60);
    let began = Instant::now();

    let Err(refused) = App::builder()
        .provide(SeaOrmConfig {
            url: url.into(),
            connect_timeout: only_a_retry_spends,
            ..Default::default()
        })
        .module::<PoolModule>()
        .build()
        .await
    else {
        panic!("a pool opened on a server it could not verify");
    };

    let connecting = logs.expect_one(nest_rs_seaorm::target::ORM, "connecting to database");
    assert_eq!(
        connecting.field("max_connections"),
        Some(SeaOrmConfig::default().max_connections.to_string()),
        "a number, never `Some(10)`",
    );

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
        began.elapsed() < only_a_retry_spends,
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

/// Runs `check` where each TLS variable names a file of its contents, for the
/// closure alone, on a runtime opened inside it, in the folder holding them.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn under_tls_variables(variables: &[(&str, &str)], check: impl AsyncFnOnce(&std::path::Path)) {
    figment::Jail::expect_with(|jail| {
        for (name, contents) in variables {
            let file = jail.directory().join(format!("{name}.pem"));
            std::fs::write(&file, contents).expect("the file");
            jail.set_env(*name, file.display());
        }
        let directory = jail.directory().to_owned();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(check(&directory));
        Ok(())
    });
}

/// How a boot on `url` ends, which each double here ends at once: `HUNG` guards
/// the wait, and only a retry could spend the budget.
async fn refused_boot(url: String) -> String {
    let Ok(Err(refused)) = tokio::time::timeout(
        HUNG,
        App::builder()
            .provide(SeaOrmConfig {
                url: url.into(),
                connect_timeout: Duration::from_secs(60),
                ..Default::default()
            })
            .module::<PoolModule>()
            .build(),
    )
    .await
    else {
        panic!("a pool opened on a double that lets no one in, or no refusal ended its proof");
    };
    format!("{refused:#}")
}

/// libpq's `PGSSLROOTCERT` names no authority here: a server only that file
/// vouches for is refused as any other, pointing at `sslrootcert`, which logs in
/// to that server when the URL names the same file.
#[test]
fn pgsslrootcert_vouches_for_no_server_and_the_urls_sslrootcert_does() {
    let authority = TestAuthority::new();
    under_tls_variables(&[("PGSSLROOTCERT", authority.pem())], async |directory| {
        let (url, started) = postgres_signed_by(&authority, None).await;
        let refused = refused_boot(url).await;
        assert!(
            !started.load(Ordering::SeqCst),
            "the pool logged in on the variable's word: {refused}"
        );
        assert!(
            refused.contains("UnknownIssuer") && refused.contains("sslrootcert"),
            "{refused}"
        );

        let (url, started) = postgres_signed_by(&authority, None).await;
        let file = directory.join("PGSSLROOTCERT.pem");
        let refused = refused_boot(format!("{url}?sslrootcert={}", file.display())).await;
        assert!(
            started.load(Ordering::SeqCst) && refused.contains(LOGIN_REFUSED),
            "{refused}"
        );
    });
}

/// libpq's `PGSSLROOTCERT` narrows nothing either: a server the system's
/// authorities vouch for is logged in to, whatever other authority the variable
/// names. `SSL_CERT_FILE` is the system's store here, read once per process.
#[test]
fn pgsslrootcert_narrows_nothing_the_systems_authorities_vouch_for() {
    let (system, other) = (TestAuthority::new(), TestAuthority::new());
    under_tls_variables(
        &[
            ("SSL_CERT_FILE", system.pem()),
            ("PGSSLROOTCERT", other.pem()),
        ],
        async |_| {
            let (url, started) = postgres_signed_by(&system, None).await;
            let refused = refused_boot(url).await;
            assert!(
                started.load(Ordering::SeqCst) && refused.contains(LOGIN_REFUSED),
                "{refused}"
            );
        },
    );
}

/// libpq's `PGSSLCERT` and `PGSSLKEY` name a client certificate beside a URL
/// naming neither, read as its `sslcert` and `sslkey` are: the double asks for
/// one, and no handshake ends without it.
#[test]
fn pgsslcert_and_pgsslkey_present_a_client_certificate() {
    let (server, clients) = (TestAuthority::new(), TestAuthority::new());
    let client = clients.client("nestrs");
    under_tls_variables(
        &[
            ("SSL_CERT_FILE", server.pem()),
            ("PGSSLCERT", &client.cert),
            ("PGSSLKEY", &client.key),
        ],
        async |_| {
            let (url, started) = postgres_signed_by(&server, Some(&clients)).await;
            let refused = refused_boot(url).await;
            assert!(
                started.load(Ordering::SeqCst) && refused.contains(LOGIN_REFUSED),
                "{refused}"
            );
        },
    );
}

/// A host whose name does not resolve yet — a database started after the app,
/// behind DNS that learns of it then — is tried again within the budget, then
/// refused as unreachable, naming the host and never the URL. `.invalid` never
/// resolves (RFC 6761).
#[tokio::test]
async fn a_host_whose_name_does_not_resolve_is_retried_within_the_budget_then_refused() {
    let logs = nest_rs_testing::LogCapture::install();
    let budget = Duration::from_secs(2);

    let Ok(Err(refused)) = tokio::time::timeout(
        HUNG,
        App::builder()
            .provide(SeaOrmConfig {
                url: "postgres://nestrs:s3cret@nestrs-boot-proof.invalid:5432/app".into(),
                connect_timeout: budget,
                ..Default::default()
            })
            .module::<PoolModule>()
            .build(),
    )
    .await
    else {
        panic!("a pool opened on a host that does not resolve, or no budget ended its proof");
    };

    let rendered = format!("{refused:#}");
    assert!(
        rendered.contains("nestrs-boot-proof.invalid:5432")
            && rendered.contains(&format!("could not be reached within {budget:?}"))
            && rendered.contains(&nest_rs_config::var_name("seaorm", "URL"))
            && rendered.contains(&nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS")),
        "unreachable, naming the host and what to change: {rendered}"
    );
    assert!(!rendered.contains("s3cret"), "{rendered}");
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

/// A Postgres double speaking no TLS and answering every startup message with
/// `answer`, on a port this returns.
async fn postgres_answering_with(answer: Vec<u8>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let port = listener.local_addr().expect("its address").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let answer = answer.clone();
            tokio::spawn(async move {
                let mut length = [0_u8; 4];
                if socket.read_exact(&mut length).await.is_err() {
                    return;
                }
                let rest = usize::try_from(u32::from_be_bytes(length))
                    .unwrap_or(0)
                    .saturating_sub(4);
                let mut startup = vec![0_u8; rest];
                if socket.read_exact(&mut startup).await.is_err()
                    || socket.write_all(&answer).await.is_err()
                {
                    return;
                }
                // Held until the client's goodbye, as a server's connection is.
                let _ = socket.read_to_end(&mut Vec::new()).await;
            });
        }
    });
    port
}

/// A Postgres double refusing every startup message with `code` in the words
/// `message` gives, on a port this returns.
async fn postgres_refusing(code: &str, message: &str) -> u16 {
    postgres_answering_with(error_response(code, message)).await
}

/// A Postgres double letting every connection in, as a server trusting its
/// clients does: `AuthenticationOk`, then `ReadyForQuery`.
async fn postgres_letting_in() -> u16 {
    let mut answer = postgres_message(b'R', &0_u32.to_be_bytes());
    answer.extend(postgres_message(b'Z', b"I"));
    postgres_answering_with(answer).await
}

/// The URL logging in to a double on `port` with `password`.
fn plaintext_url(password: &str, port: u16) -> String {
    format!("postgres://nestrs:{password}@127.0.0.1:{port}/postgres?sslmode=disable")
}

/// Boots a pool on `url` with a 1 s connect budget, which must refuse it as
/// unreachable after retrying, and hands back the refusal as the framework
/// says it and the `error` field of every retry line.
async fn unreachable_after_retrying(url: String) -> (String, Vec<String>) {
    let logs = nest_rs_testing::LogCapture::install();
    let budget = Duration::from_secs(1);
    let Ok(Err(refused)) = tokio::time::timeout(
        HUNG,
        App::builder()
            .provide(SeaOrmConfig {
                url: url.into(),
                connect_timeout: budget,
                ..Default::default()
            })
            .module::<PoolModule>()
            .build(),
    )
    .await
    else {
        panic!("a pool opened on a server that let no connection in, or no budget ended its proof");
    };
    let rendered = nest_rs_core::error_message(&*refused);
    assert!(
        rendered.contains(&format!("could not be reached within {budget:?}"))
            && !rendered.contains("s3cret"),
        "the budget ended the proof: {rendered}"
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
    let (rendered, errors) = unreachable_after_retrying(plaintext_url("s3cret", port)).await;

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
        let port =
            postgres_refusing(code, "the database system cannot accept connections yet").await;
        let (rendered, errors) = unreachable_after_retrying(plaintext_url("s3cret", port)).await;

        assert!(rendered.contains(code), "{code}: {rendered}");
        for error in errors {
            assert!(error.contains(code), "{code}: {error}");
        }
    }
}

#[expect(
    dead_code,
    reason = "the pool is declared for the container to resolve, never read"
)]
#[nest_rs_core::injectable]
struct InjectsThePool {
    #[inject]
    db: Arc<sea_orm::DatabaseConnection>,
}

#[module(providers = [InjectsThePool])]
struct NoPoolModule;

#[tokio::test]
async fn a_provider_injecting_a_pool_no_module_opens_is_refused_naming_the_module() {
    let Err(refused) = App::builder().module::<NoPoolModule>().build().await else {
        panic!("no module opens the pool");
    };
    let missing = refused
        .downcast::<nest_rs_core::MissingDependencyError>()
        .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(missing.consumer, "InjectsThePool");
    assert!(
        missing
            .remedy
            .is_some_and(|remedy| remedy.contains("`nest_rs::seaorm::SeaOrmModule`")),
        "{missing}"
    );
}

/// What marks the URL's password in anything that prints.
const PASSWORD: &str = "zzqx-password";

/// The boot on `url`, which must be refused within a 1 s budget, as everything
/// it prints and every line it files; neither may quote [`PASSWORD`].
async fn refused_quoting_no_password(case: &str, url: String) -> String {
    let logs = nest_rs_testing::LogCapture::install();
    let Ok(Err(refused)) = tokio::time::timeout(
        HUNG,
        App::builder()
            .provide(SeaOrmConfig {
                url: url.into(),
                connect_timeout: Duration::from_secs(1),
                ..Default::default()
            })
            .module::<PoolModule>()
            .build(),
    )
    .await
    else {
        panic!("{case}: a pool opened, or no budget ended its proof");
    };
    let printed = nest_rs_testing::printed(&*refused);
    assert!(!printed.contains(PASSWORD), "{case}: {printed}");
    assert!(!logs.quotes(PASSWORD), "{case}: {:#?}", logs.events());
    printed
}

/// What a `main` returning the boot's refusal prints is every link as it is:
/// no failure of a URL holding a password quotes it, whether the server is
/// silent, unnamed or refusing.
#[tokio::test]
async fn no_refused_boot_prints_the_urls_password() {
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        listener.local_addr().expect("its address").port()
    };
    let refused_password = postgres_refusing(
        "28P01",
        "password authentication failed for user \"nestrs\"",
    )
    .await;
    let no_database = postgres_refusing("3D000", "database \"postgres\" does not exist").await;
    let starting = postgres_refusing("57P03", "the database system is starting up").await;
    let (untrusted, _) = postgres_signed_by(&TestAuthority::new(), None).await;
    let cases = [
        ("nothing listens", plaintext_url(PASSWORD, closed)),
        (
            "no such host",
            format!("postgres://nestrs:{PASSWORD}@no-such-host.invalid/postgres?sslmode=disable"),
        ),
        (
            "a wrong password",
            plaintext_url(PASSWORD, refused_password),
        ),
        ("no such database", plaintext_url(PASSWORD, no_database)),
        ("a server starting up", plaintext_url(PASSWORD, starting)),
        (
            "a certificate no authority signed",
            untrusted.replace("s3cret", PASSWORD),
        ),
        (
            "a URL that does not parse",
            format!("postgres://nestrs:{PASSWORD}@127.0.0.1:port/postgres"),
        ),
        (
            "a mode sqlx does not know",
            format!("postgres://nestrs:{PASSWORD}@127.0.0.1:1/postgres?sslmode=zzqx-mode"),
        ),
        (
            "a mode that does not verify",
            format!("postgres://nestrs:{PASSWORD}@127.0.0.1:1/postgres?sslmode=require"),
        ),
    ];
    for (case, url) in cases {
        refused_quoting_no_password(case, url).await;
    }
}

/// sea-orm names the driver by the URL's scheme, and quotes the whole URL when it
/// knows none; sqlx, which the boot's proof dials through, never reads the
/// scheme, so a server letting the proof in is what reaches that sentence.
#[tokio::test]
async fn a_scheme_naming_no_driver_is_refused_without_its_password() {
    let port = postgres_letting_in().await;
    for scheme in ["postgress", "mysql", "http"] {
        let printed = refused_quoting_no_password(
            scheme,
            format!("{scheme}://nestrs:{PASSWORD}@127.0.0.1:{port}/postgres?sslmode=disable"),
        )
        .await;
        assert!(
            printed.contains(&nest_rs_config::var_name("seaorm", "URL"))
                && printed.contains("is not a Postgres URL"),
            "{scheme}: {printed}"
        );
    }
}
