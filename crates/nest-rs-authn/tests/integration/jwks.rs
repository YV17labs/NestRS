//! Covers `src/jwks.rs` — an issuer's JWK Set, served over TLS by an in-process
//! double whose certificate a `TestAuthority` issued, verified through
//! `JwtService` and through a booted guard.
//!
//! The clock moves past the refresh floor only between fetches (`elapse`): a
//! fetch is real I/O, and a paused clock would jump its deadline.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{
    AlgorithmParameters, CommonParameters, EllipticCurve, Jwk, OctetKeyPairParameters,
    OctetKeyPairType, PublicKeyUse,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header, get_current_timestamp};
use nest_rs_authn::{
    AuthError, AuthnConfig, AuthnGuard, AuthnModule, AuthnTls, JWKS_MAX_BYTES, JWKS_REFRESH_FLOOR,
    JWKS_STALE_CEILING, JwtKey, JwtOptions, JwtService, JwtStrategy, PrincipalIdentity,
};
use nest_rs_core::module;
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_testing::{LogCapture, TestApp, TestAuthority, TestCertificate, wait_until};
use poem::http::{StatusCode, header};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinSet;

/// The authority the double's certificate chains to, and the client trusts.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// A key the issuer signs with, and the public JWK it publishes for it.
struct Signing {
    key: EncodingKey,
    public: Jwk,
}

impl std::ops::Deref for Signing {
    type Target = EncodingKey;

    fn deref(&self) -> &EncodingKey {
        &self.key
    }
}

/// The issuer's signing keys, one per key type, generated once per process.
static RSA: LazyLock<Signing> = LazyLock::new(|| {
    let pair = rcgen::KeyPair::generate_rsa_for(&rcgen::PKCS_RSA_SHA256, rcgen::RsaKeySize::_2048)
        .expect("an RSA key generates");
    let key = EncodingKey::from_rsa_pem(pair.serialize_pem().as_bytes()).expect("a PKCS#8 key");
    let public = Jwk::from_encoding_key(&key, Algorithm::RS256).expect("its public JWK");
    Signing { key, public }
});

static P256: LazyLock<Signing> = LazyLock::new(|| {
    let pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .expect("a P-256 key generates");
    let key = EncodingKey::from_ec_pem(pair.serialize_pem().as_bytes()).expect("a PKCS#8 key");
    let public = Jwk::from_encoding_key(&key, Algorithm::ES256).expect("its public JWK");
    Signing { key, public }
});

/// Built by hand: `Jwk::from_encoding_key` reads only a PKCS#8 v1 Ed25519 key,
/// and aws-lc writes v2.
static ED25519: LazyLock<Signing> = LazyLock::new(|| {
    let pair =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("an Ed25519 key generates");
    let key = EncodingKey::from_ed_pem(pair.serialize_pem().as_bytes()).expect("a PKCS#8 key");
    let public = Jwk {
        common: CommonParameters::default(),
        algorithm: AlgorithmParameters::OctetKeyPair(OctetKeyPairParameters {
            key_type: OctetKeyPairType::OctetKeyPair,
            curve: EllipticCurve::Ed25519,
            x: URL_SAFE_NO_PAD.encode(pair.public_key_raw()),
        }),
    };
    Signing { key, public }
});

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: u64,
}

impl PrincipalIdentity for Claims {
    fn actor_id(&self) -> Option<String> {
        Some(self.sub.clone())
    }
}

/// The public JWK of `key`, published under `kid` for `algorithm`.
fn jwk(key: &Signing, algorithm: Algorithm, kid: &str) -> Jwk {
    let mut jwk = key.public.clone();
    jwk.common.key_algorithm = Some(algorithm.into());
    jwk.common.key_id = Some(kid.to_owned());
    jwk
}

/// `jwk` without the `alg` member, as an issuer that leaves it out publishes it.
fn unbound(mut jwk: Jwk) -> Jwk {
    jwk.common.key_algorithm = None;
    jwk
}

/// An access token `key` signed with `algorithm`, naming `kid`.
fn token(key: &EncodingKey, algorithm: Algorithm, kid: Option<&str>) -> String {
    let mut header = Header::new(algorithm);
    header.kid = kid.map(str::to_owned);
    header.typ = Some("at+jwt".into());
    let claims = Claims {
        sub: "ada".into(),
        exp: get_current_timestamp() + 3600,
    };
    jsonwebtoken::encode(&header, &claims, key).expect("the token signs")
}

/// What the double answers every fetch with.
#[derive(Clone)]
struct Answer {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    /// Whether the answer states its length, or ends when the connection does.
    length: bool,
}

impl Answer {
    fn keys(keys: &[Jwk]) -> Self {
        Self::body(serde_json::to_vec(&serde_json::json!({ "keys": keys })).expect("JSON"))
    }

    fn body(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type", "application/jwk-set+json".to_owned())],
            body: body.into(),
            length: true,
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            length: true,
        }
    }

    fn with(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }

    fn until_closed(mut self) -> Self {
        self.length = false;
        self
    }
}

/// An issuer's JWK Set endpoint: HTTPS on loopback, one answer per
/// connection, every fetch counted.
struct Issuer {
    addr: SocketAddr,
    answer: Arc<Mutex<Answer>>,
    fetches: Arc<AtomicUsize>,
}

impl Issuer {
    async fn serving(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("its address");
        let answer = Arc::new(Mutex::new(answer));
        let fetches = Arc::new(AtomicUsize::new(0));
        let acceptor = CERTIFICATE.acceptor(None);
        let (served, counted) = (Arc::clone(&answer), Arc::clone(&fetches));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, answer, fetches) =
                    (acceptor.clone(), Arc::clone(&served), Arc::clone(&counted));
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut head = Vec::new();
                    let mut buf = [0_u8; 1024];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match tls.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => head.extend_from_slice(&buf[..read]),
                        }
                    }
                    fetches.fetch_add(1, Ordering::SeqCst);
                    let answer = answer.lock().expect("the answer").clone();
                    let mut response =
                        format!("HTTP/1.1 {} Answer\r\nconnection: close\r\n", answer.status);
                    if answer.length {
                        response.push_str(&format!("content-length: {}\r\n", answer.body.len()));
                    }
                    for (name, value) in &answer.headers {
                        response.push_str(&format!("{name}: {value}\r\n"));
                    }
                    response.push_str("\r\n");
                    let _ = tls.write_all(response.as_bytes()).await;
                    let _ = tls.write_all(&answer.body).await;
                    let _ = tls.shutdown().await;
                });
            }
        });
        Self {
            addr,
            answer,
            fetches,
        }
    }

    fn uri(&self) -> String {
        format!("https://127.0.0.1:{}/api/auth/jwks", self.addr.port())
    }

    fn answer(&self, answer: Answer) {
        *self.answer.lock().expect("the answer") = answer;
    }

    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }
}

/// The test authority, for a client to trust.
fn authority() -> Vec<u8> {
    AUTHORITY.pem().as_bytes().to_vec()
}

/// A resource server verifying `uri`'s set, trusting the test authority.
fn verifier_of(uri: String, narrow: impl FnOnce(&mut JwtOptions)) -> JwtService {
    let mut options = JwtOptions::jwks(uri.clone());
    options.key = JwtKey::Jwks {
        uri,
        ca_cert: Some(authority()),
    };
    narrow(&mut options);
    JwtService::new(options).expect("a JWK Set verifier")
}

fn verifier(issuer: &Issuer) -> JwtService {
    verifier_of(issuer.uri(), |_| {})
}

/// A URI on loopback nothing listens on.
fn unreachable() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let port = listener.local_addr().expect("its address").port();
    format!("https://127.0.0.1:{port}/jwks")
}

/// Let `by` pass on tokio's clock, with no fetch in flight, so no I/O deadline
/// moves with it.
async fn elapse(by: Duration) {
    tokio::time::pause();
    tokio::time::advance(by).await;
    tokio::time::resume();
}

/// What `token` verifies to, as the reason it does not.
async fn verdict(jwt: &JwtService, token: &str) -> Result<Claims, AuthError> {
    jwt.verify::<Claims>(token).await
}

#[tokio::test]
async fn rsa_ec_and_ed25519_tokens_verify_against_the_set() {
    let issuer = Issuer::serving(Answer::keys(&[
        unbound(jwk(&RSA, Algorithm::RS256, "rsa")),
        jwk(&P256, Algorithm::ES256, "p256"),
        jwk(&ED25519, Algorithm::EdDSA, "ed"),
    ]))
    .await;
    let jwt = verifier(&issuer);

    for (key, algorithm, kid) in [
        (&*RSA, Algorithm::RS256, "rsa"),
        (&*RSA, Algorithm::PS256, "rsa"),
        (&*P256, Algorithm::ES256, "p256"),
        (&*ED25519, Algorithm::EdDSA, "ed"),
    ] {
        let claims = verdict(&jwt, &token(key, algorithm, Some(kid)))
            .await
            .unwrap_or_else(|refused| panic!("{algorithm:?} must verify: {refused:?}"));
        assert_eq!(claims.sub, "ada");
    }
    assert_eq!(issuer.fetches(), 1, "a fresh set serves every token");
}

/// RFC 8725 §3.1: the token's header picks among what the verifier binds each
/// key to, and nothing else.
#[tokio::test]
async fn a_key_checks_no_algorithm_but_its_own() {
    let issuer = Issuer::serving(Answer::keys(&[
        jwk(&RSA, Algorithm::RS256, "rsa"),
        jwk(&ED25519, Algorithm::EdDSA, "ed"),
    ]))
    .await;
    let jwt = verifier(&issuer);

    for (forged, why) in [
        (
            token(&RSA, Algorithm::PS256, Some("rsa")),
            "the key's own alg is RS256",
        ),
        (
            token(&RSA, Algorithm::RS256, Some("ed")),
            "an Ed25519 key checks no RSA signature",
        ),
    ] {
        assert!(
            matches!(
                verdict(&jwt, &forged).await,
                Err(AuthError::InvalidAlgorithm)
            ),
            "{why}"
        );
    }

    let narrowed = verifier_of(issuer.uri(), |options| {
        options.algorithms = vec![Algorithm::EdDSA];
    });
    assert!(matches!(
        verdict(&narrowed, &token(&RSA, Algorithm::RS256, Some("rsa"))).await,
        Err(AuthError::InvalidAlgorithm)
    ));
    verdict(&narrowed, &token(&ED25519, Algorithm::EdDSA, Some("ed")))
        .await
        .expect("the one accepted algorithm verifies");
}

/// The classic confusion — the issuer's public key, read off its own set, as an
/// HMAC secret — and an unsigned token, both refused before the set is asked
/// for.
#[tokio::test]
async fn an_hmac_or_unsigned_token_is_refused_without_asking_the_issuer() {
    let published = jwk(&RSA, Algorithm::RS256, "rsa");
    let issuer = Issuer::serving(Answer::keys(std::slice::from_ref(&published))).await;
    let jwt = verifier(&issuer);

    let as_secret = serde_json::to_vec(&published).expect("JSON");
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("rsa".into());
    header.typ = Some("at+jwt".into());
    let claims = Claims {
        sub: "mallory".into(),
        exp: get_current_timestamp() + 3600,
    };
    let confused = jsonwebtoken::encode(&header, &claims, &EncodingKey::from_secret(&as_secret))
        .expect("the forgery signs");
    assert!(matches!(
        verdict(&jwt, &confused).await,
        Err(AuthError::InvalidAlgorithm)
    ));

    let unsigned = format!(
        "{}.{}.",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"none","typ":"at+jwt","kid":"rsa"}"#),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("JSON")),
    );
    assert!(verdict(&jwt, &unsigned).await.is_err());
    assert_eq!(
        issuer.fetches(),
        0,
        "a token no accepted algorithm signs costs the issuer nothing"
    );
}

#[tokio::test]
async fn a_key_published_for_encryption_checks_no_token() {
    let mut encryption = unbound(jwk(&RSA, Algorithm::RS256, "enc"));
    encryption.common.public_key_use = Some(PublicKeyUse::Encryption);
    let issuer = Issuer::serving(Answer::keys(&[
        encryption,
        jwk(&ED25519, Algorithm::EdDSA, "ed"),
    ]))
    .await;
    let jwt = verifier(&issuer);

    assert!(matches!(
        verdict(&jwt, &token(&RSA, Algorithm::RS256, Some("enc"))).await,
        Err(AuthError::UnknownKey)
    ));
    assert_eq!(
        issuer.fetches(),
        1,
        "the set just fetched is not fetched again"
    );
}

/// A token naming no `kid` is checked by the one key that fits, and refused
/// where several do rather than tried against each.
#[tokio::test]
async fn a_token_without_a_kid_needs_exactly_one_fitting_key() {
    let issuer = Issuer::serving(Answer::keys(&[
        jwk(&ED25519, Algorithm::EdDSA, "ed"),
        jwk(&P256, Algorithm::ES256, "p256"),
    ]))
    .await;
    let jwt = verifier(&issuer);
    verdict(&jwt, &token(&ED25519, Algorithm::EdDSA, None))
        .await
        .expect("one Ed25519 key in the set");

    issuer.answer(Answer::keys(&[
        jwk(&ED25519, Algorithm::EdDSA, "ed-1"),
        jwk(&ED25519, Algorithm::EdDSA, "ed-2"),
    ]));
    let other = verifier(&issuer);
    assert!(matches!(
        verdict(&other, &token(&ED25519, Algorithm::EdDSA, None)).await,
        Err(AuthError::UnknownKey)
    ));
}

/// A rotation reaches the verifier through the one fetch a new `kid` earns,
/// once the floor since the last fetch has passed.
#[tokio::test]
async fn a_kid_the_set_lacks_is_found_after_one_refetch() {
    let issuer = Issuer::serving(Answer::keys(&[jwk(&ED25519, Algorithm::EdDSA, "old")])).await;
    let jwt = verifier(&issuer);
    verdict(&jwt, &token(&ED25519, Algorithm::EdDSA, Some("old")))
        .await
        .expect("the published key");

    issuer.answer(Answer::keys(&[
        jwk(&ED25519, Algorithm::EdDSA, "old"),
        jwk(&P256, Algorithm::ES256, "new"),
    ]));
    let rotated = token(&P256, Algorithm::ES256, Some("new"));
    assert!(
        matches!(verdict(&jwt, &rotated).await, Err(AuthError::UnknownKey)),
        "within the floor the set is not asked again"
    );
    assert_eq!(issuer.fetches(), 1);

    elapse(JWKS_REFRESH_FLOOR).await;
    verdict(&jwt, &rotated)
        .await
        .expect("the key the issuer rotated in");
    assert_eq!(issuer.fetches(), 2);
}

/// Tokens naming kids nobody published cost the issuer one fetch per floor,
/// however many arrive at once.
#[tokio::test]
async fn a_burst_of_unknown_kids_costs_one_fetch_per_floor() {
    let issuer = Issuer::serving(Answer::keys(&[jwk(&ED25519, Algorithm::EdDSA, "ed")])).await;
    let jwt = Arc::new(verifier(&issuer));
    verdict(&jwt, &token(&ED25519, Algorithm::EdDSA, Some("ed")))
        .await
        .expect("the published key");

    for (round, fetches) in [(1, 1), (2, 2)] {
        if round == 2 {
            elapse(JWKS_REFRESH_FLOOR).await;
        }
        let mut burst = JoinSet::new();
        for forged in 0..50 {
            let jwt = Arc::clone(&jwt);
            let token = token(
                &ED25519,
                Algorithm::EdDSA,
                Some(&format!("forged-{round}-{forged}")),
            );
            burst.spawn(async move { jwt.verify::<Claims>(&token).await });
        }
        for outcome in burst.join_all().await {
            assert!(matches!(outcome, Err(AuthError::UnknownKey)), "{outcome:?}");
        }
        assert_eq!(issuer.fetches(), fetches, "round {round}");
    }
}

/// The answer's `max-age` is when the set is asked for again; a stale set
/// keeps serving while it is.
#[tokio::test]
async fn a_set_is_fetched_again_once_its_max_age_runs_out() {
    let issuer = Issuer::serving(
        Answer::keys(&[jwk(&ED25519, Algorithm::EdDSA, "ed")]).with("cache-control", "max-age=120"),
    )
    .await;
    let jwt = verifier(&issuer);
    let token = token(&ED25519, Algorithm::EdDSA, Some("ed"));
    verdict(&jwt, &token).await.expect("fetched");

    elapse(Duration::from_secs(119)).await;
    verdict(&jwt, &token).await.expect("still fresh");
    assert_eq!(issuer.fetches(), 1);

    elapse(Duration::from_secs(2)).await;
    verdict(&jwt, &token)
        .await
        .expect("served stale while it refreshes");
    wait_until(Duration::from_secs(5), || issuer.fetches() == 2).await;
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_last_good_set_until_its_stale_ceiling() {
    let logs = LogCapture::install();
    let issuer = Issuer::serving(
        Answer::keys(&[jwk(&ED25519, Algorithm::EdDSA, "ed")]).with("cache-control", "max-age=60"),
    )
    .await;
    let jwt = verifier(&issuer);
    let valid = token(&ED25519, Algorithm::EdDSA, Some("ed"));
    verdict(&jwt, &valid).await.expect("fetched");

    issuer.answer(Answer::status(500));
    elapse(Duration::from_secs(61)).await;
    verdict(&jwt, &valid)
        .await
        .expect("the last good set still verifies");
    let kept = "the issuer's key set could not be refreshed; the last good one is kept";
    wait_until(Duration::from_secs(5), || {
        !logs.find(nest_rs_authn::TARGET, kept).is_empty()
    })
    .await;
    let event = logs.expect_one(nest_rs_authn::TARGET, kept);
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("error")
            .is_some_and(|error| error.contains("answered 500")),
        "{:?}",
        event.fields
    );
    assert!(
        matches!(
            verdict(&jwt, &token(&ED25519, Algorithm::EdDSA, Some("other"))).await,
            Err(AuthError::Unavailable { .. })
        ),
        "a kid looked for while the issuer fails was never checked"
    );

    elapse(JWKS_STALE_CEILING).await;
    let Err(AuthError::Unavailable { detail, .. }) = verdict(&jwt, &valid).await else {
        panic!("past its stale ceiling the set verifies nothing");
    };
    assert!(detail.contains("answered 500"), "{detail}");
}

/// An issuer that answers anything but a JWK Set is an outage, never a bad
/// token — and what it answered is never quoted.
#[tokio::test]
async fn an_answer_that_is_not_a_usable_set_is_unavailable_and_never_quoted() {
    const SENTINEL: &str = "never-quoted-7f3a";
    let oversized = vec![b' '; JWKS_MAX_BYTES + 1];
    for (answer, reason, retry_after) in [
        (
            Answer::body(format!(r#"{{"keys":"{SENTINEL}"}}"#)),
            "is not a JWK Set",
            None,
        ),
        (
            Answer::body(format!("<html>{SENTINEL}</html>")),
            "is not a JWK Set",
            None,
        ),
        (Answer::body(oversized.clone()), "more than", None),
        (Answer::body(oversized).until_closed(), "more than", None),
        (
            Answer::keys(&[]),
            "holds no key this verifier can use",
            None,
        ),
        (
            Answer::status(302).with("location", "https://elsewhere.example/jwks"),
            "a redirect is not followed",
            None,
        ),
        (
            Answer::status(503).with("retry-after", "120"),
            "answered 503",
            Some(Duration::from_secs(120)),
        ),
    ] {
        let issuer = Issuer::serving(answer).await;
        let jwt = verifier(&issuer);
        let Err(AuthError::Unavailable {
            detail,
            retry_after: waited,
        }) = verdict(&jwt, &token(&ED25519, Algorithm::EdDSA, Some("ed"))).await
        else {
            panic!("{reason}: must be unavailable");
        };
        assert!(detail.contains(reason), "{detail}");
        assert!(!detail.contains(SENTINEL), "{detail}");
        assert!(!detail.contains("elsewhere"), "{detail}");
        if retry_after.is_some() {
            assert_eq!(waited, retry_after, "the issuer's own wait");
        } else {
            assert!(waited.is_some(), "the wait until the next fetch may start");
        }
        assert_eq!(issuer.fetches(), 1);
    }
}

// The set through the module's seam, behind a booted guard.

type JwksStrategy = JwtStrategy<Claims>;
type JwksGuard = AuthnGuard<JwksStrategy>;

const REACHED: &str = "reached";

#[controller(path = "/posts")]
#[use_guards(JwksGuard)]
struct PostsController;

#[routes]
impl PostsController {
    #[get("/mine")]
    async fn mine(&self) -> &'static str {
        REACHED
    }

    #[get("/latest")]
    #[public]
    async fn latest(&self) -> &'static str {
        REACHED
    }
}

#[module(
    imports = [
        HttpModule::for_root(HttpConfig { port: 0, ..Default::default() }),
        AuthnModule::for_root(None),
    ],
    providers = [JwksStrategy, JwksGuard, PostsController],
)]
struct JwksHttpModule;

/// The module boots on the config a deployment gives it — seeded here, as a
/// hermetic test seeds one.
async fn app(jwks_uri: String) -> TestApp {
    TestApp::builder()
        .module::<JwksHttpModule>()
        .provide(AuthnConfig {
            jwks_uri: Some(jwks_uri),
            tls: AuthnTls {
                ca_cert: Some(authority()),
            },
            ..AuthnConfig::default()
        })
        .build()
        .await
        .expect("the app boots without fetching the set")
}

#[tokio::test]
async fn a_guarded_route_admits_a_token_the_issuers_set_verifies() {
    let issuer = Issuer::serving(Answer::keys(&[jwk(&ED25519, Algorithm::EdDSA, "ed")])).await;
    let app = app(issuer.uri()).await;
    assert_eq!(issuer.fetches(), 0, "nothing is fetched at boot");

    let admitted = app
        .http()
        .get("/posts/mine")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token(&ED25519, Algorithm::EdDSA, Some("ed"))),
        )
        .send()
        .await;
    admitted.assert_status_is_ok();
    admitted.assert_text(REACHED).await;

    let foreign = app
        .http()
        .get("/posts/mine")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token(&P256, Algorithm::ES256, Some("p256"))),
        )
        .send()
        .await;
    foreign.assert_status(StatusCode::UNAUTHORIZED);
}

/// A set that cannot be had is the `503` an unreachable store is, on a
/// `#[public]` route as on a guarded one: the token was never checked.
#[tokio::test]
async fn an_unreachable_set_answers_503_on_a_guarded_and_a_public_route() {
    let logs = LogCapture::install();
    let app = app(unreachable()).await;
    let bearer = format!("Bearer {}", token(&ED25519, Algorithm::EdDSA, Some("ed")));

    for path in ["/posts/mine", "/posts/latest"] {
        let refused = app
            .http()
            .get(path)
            .header(header::AUTHORIZATION, bearer.as_str())
            .send()
            .await;
        refused.assert_status(StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            refused.0.headers().get(header::RETRY_AFTER).is_some(),
            "{path}: when the next fetch may start"
        );
        assert!(
            refused.0.headers().get(header::WWW_AUTHENTICATE).is_none(),
            "{path}: no challenge blames the token"
        );
    }
    let outages = logs.find(
        nest_rs_authn::TARGET,
        "authentication unavailable — what the strategy asks did not answer",
    );
    assert_eq!(outages.len(), 2);
    assert!(
        outages[0]
            .field("error")
            .is_some_and(|error| error.contains("could not be fetched")),
        "{:?}",
        outages[0].fields
    );

    app.http()
        .get("/posts/latest")
        .send()
        .await
        .assert_status_is_ok();
}
