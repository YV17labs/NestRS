//! Covers `src/module.rs`: the pool an app opens verifies the server's
//! certificate, and logs in to none it cannot verify.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

#[tokio::test]
async fn the_pool_never_logs_in_to_a_server_whose_certificate_no_trusted_authority_signed() {
    let (url, started) = untrusted_postgres().await;

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
}
