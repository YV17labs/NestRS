//! `rediss://` without a Redis: a certificate the client does not accept is
//! refused at the handshake, before the proxy would forward a byte, and a first
//! `HELLO` nothing answers is asked again on a real connection.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nest_rs_config::ClientTls;
use nest_rs_redis::{RedisConfig, RedisConnection, RedisError};
use nest_rs_testing::LogCapture;
use tokio::net::TcpListener;

use crate::harness::connection::answer_each_command;
use crate::harness::no_attempt_retried;
use crate::harness::tls::{self, TlsProxy, config, trusting_the_test_authority};
use crate::{HELLO, is_hello};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_certificate_the_client_does_not_accept_fails_the_boot_at_once() {
    let logs = LogCapture::install();
    let untrusted = TlsProxy::start(None, None).await;
    let misnamed = TlsProxy::start(None, None).await;
    misnamed.present_a_certificate_issued_for_another_host();

    for (case, proxy, tls, fixed_by, says) in [
        (
            "signed by an authority the client does not trust",
            &untrusted,
            ClientTls::default(),
            "TLS_CA_CERT",
            "does not chain to an authority",
        ),
        (
            "issued for a name the URL does not dial",
            &misnamed,
            trusting_the_test_authority(),
            "URL",
            "subjectAltName",
        ),
    ] {
        let Err(error) = RedisConnection::connect(&config(proxy.url_on(0), tls)).await else {
            panic!("a certificate {case} must not connect")
        };
        assert!(
            matches!(error, RedisError::TlsRefused { .. }),
            "{case}: {error}"
        );
        no_attempt_retried(&logs, case);
        assert_eq!(proxy.handshakes(), 1, "{case}: one attempt, refused");
        let rendered = error.to_string();
        assert!(
            rendered.contains(&nest_rs_config::var_name("redis", fixed_by)),
            "{case}: the error names the setting that fixes it: {error}",
        );
        assert!(
            rendered.contains(says),
            "{case}: and gives the cause that setting fixes, not another's: {error}",
        );
    }
}

/// A `HELLO` sent as its connection's driver ends is stranded — neither
/// answered nor dropped — and only the next command on that connection meets
/// the close. No double strands one: the window lies inside `redis`'s send,
/// between the permit it reserves and the message it pushes, on the caller's
/// thread. So this server does to the boot what a stranded `HELLO` does: it
/// leaves its first connection's first `HELLO` unanswered and drops that
/// connection at the next command. The boot asks again at a quarter of its
/// budget, hears the drop, dials again and opens — where a `HELLO` asked once
/// waits out the budget and calls the server unreachable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_hello_nothing_answers_is_asked_again_and_the_boot_opens_on_a_dial_of_its_own() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the server");
    let addr = listener.local_addr().expect("the server's address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let accepting = Arc::clone(&accepted);
    let serving = tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            let id = accepting.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let Some(client) = tls::accept(client).await else {
                    return;
                };
                let mut hellos = 0;
                answer_each_command(client, |command| -> Option<&'static [u8]> {
                    if !is_hello(command) {
                        return Some(b"+PONG\r\n");
                    }
                    hellos += 1;
                    match (id, hellos) {
                        (0, 1) => Some(b""),
                        (0, _) => None,
                        _ => Some(HELLO),
                    }
                })
                .await;
            });
        }
    });

    let opened = RedisConnection::connect(&RedisConfig {
        url: format!("rediss://{addr}/").into(),
        connect_timeout: Duration::from_secs(4),
        tls: trusting_the_test_authority(),
    })
    .await;
    serving.abort();

    opened.unwrap_or_else(|refused| panic!("the boot opens on a dial of its own: {refused}"));
    assert!(
        accepted.load(Ordering::SeqCst) >= 2,
        "the connection whose `HELLO` went unanswered is not the one the boot opened"
    );
}
