//! `src/sessions.rs`: a session is served to the principal that opened it
//! alone. Another principal's request naming it — a `POST`, the standalone
//! stream, a resume, a `DELETE` — reads what a session the mount does not
//! hold reads, here and on an instance that restores it from the app's store.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nest_rs_authn::{AuthError, AuthnGuard, PrincipalIdentity, Strategy, bearer_token};
use nest_rs_core::{AmbientValue, injectable, module};
use nest_rs_guards::guard;
use nest_rs_http::{Request, StatusCode};
use nest_rs_mcp::rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
use nest_rs_mcp::{McpError, mcp, tools};
use nest_rs_testing::mcp::{call_method, open_session, post_message};
use nest_rs_testing::{LogCapture, TestApp, TestResponse};
use serde_json::json;
use tokio::sync::{Notify, Semaphore};

use crate::MemoryStore;

const PATH: &str = "/mcp/owned";

/// The line a request on another principal's session files.
const REFUSED: &str = "mcp session refused: another principal opened it";

/// Who a bearer names: `<actor>`, `<actor>@<tenant>`, or `-@<tenant>` for a
/// principal with no actor id of its own, as a machine client's is.
#[derive(Clone, Debug)]
struct Named {
    actor: Option<String>,
    tenant: Option<String>,
}

impl AmbientValue for Named {}

impl PrincipalIdentity for Named {
    fn actor_id(&self) -> Option<String> {
        self.actor.clone()
    }

    fn tenant_id(&self) -> Option<String> {
        self.tenant.clone()
    }
}

/// Authenticates whoever the bearer names.
#[injectable]
#[derive(Default)]
struct ByName;

#[nest_rs_http::async_trait]
impl Strategy for ByName {
    type Principal = Named;

    async fn authenticate(&self, req: &mut Request) -> Result<Named, AuthError> {
        let named = bearer_token(req.headers()).ok_or(AuthError::MissingCredentials)?;
        let (actor, tenant) = match named.split_once('@') {
            Some((actor, tenant)) => (actor, Some(tenant.to_owned())),
            None => (named, None),
        };
        Ok(Named {
            actor: (actor != "-").then(|| actor.to_owned()),
            tenant,
        })
    }
}

type ByNameAuthn = AuthnGuard<ByName>;

#[mcp(path = "/mcp/owned")]
#[derive(Clone, Default)]
struct OwnedTools;

#[tools]
impl OwnedTools {
    #[tool(description = "Answers at once.")]
    #[public]
    async fn ping(&self) -> Result<String, McpError> {
        Ok("pong".to_owned())
    }
}

#[module(providers = [ByName, ByNameAuthn, OwnedTools])]
struct GatedModule;

#[module(imports = [crate::open_mcp()], providers = [ByName, ByNameAuthn, OwnedTools])]
struct PublicModule;

/// `M` behind a pooled [`ByNameAuthn`], restoring its sessions from `store`
/// when it has one.
async fn serving<M: nest_rs_core::Module + 'static>(store: Option<&Arc<MemoryStore>>) -> TestApp {
    let mut builder = TestApp::builder().module::<M>();
    if let Some(store) = store {
        builder =
            builder.provide_dyn::<dyn SessionStore>(Arc::clone(store) as Arc<dyn SessionStore>);
    }
    builder
        .use_guards_global([guard::<ByNameAuthn>()])
        .build()
        .await
        .expect("an #[mcp] host under a pooled AuthnGuard boots")
}

fn bearer(named: &str) -> String {
    format!("Bearer {named}")
}

/// `tools/list` on `session`, as whoever `named` says, or anonymously.
async fn list(app: &TestApp, session: &str, named: Option<&str>) -> TestResponse {
    post_message(
        app.http(),
        PATH,
        Some(session),
        named.map(bearer).as_deref(),
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await
}

/// Whether `session` still answers `named` its tool list.
async fn serves(app: &TestApp, session: &str, named: Option<&str>) -> bool {
    let body = call_method(
        app.http(),
        PATH,
        session,
        named.map(bearer).as_deref(),
        "tools/list",
        json!({}),
    )
    .await;
    body.contains("ping")
}

/// The standalone stream of `session` — resumed after `last_event` when
/// given — as whoever `named` says.
async fn stream(
    app: &TestApp,
    session: &str,
    named: &str,
    last_event: Option<&str>,
) -> TestResponse {
    let mut request = app
        .http()
        .get(PATH)
        .header("host", "localhost")
        .header("accept", "text/event-stream")
        .header("mcp-session-id", session)
        .header("authorization", bearer(named));
    if let Some(last_event) = last_event {
        request = request.header("last-event-id", last_event);
    }
    request.send().await
}

/// A `DELETE` of `session` as whoever `named` says.
async fn delete(app: &TestApp, session: &str, named: &str) -> TestResponse {
    app.http()
        .delete(PATH)
        .header("host", "localhost")
        .header("mcp-session-id", session)
        .header("authorization", bearer(named))
        .send()
        .await
}

/// What a client reads of an answer: its status, its type and its body.
async fn read(response: TestResponse) -> (StatusCode, Option<String>, String) {
    let status = response.0.status();
    let content_type = response
        .0
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.0.into_body().into_string().await.expect("a body");
    (status, content_type, body)
}

/// `refused` reads exactly as an unknown session's answer to the same call:
/// `404`, rmcp's own sentence, nothing telling the two apart.
async fn assert_answered_as_unknown(refused: TestResponse, unknown: TestResponse) {
    // Before the body: a stream served to the wrong caller never ends.
    refused.assert_status(StatusCode::NOT_FOUND);
    let refused = read(refused).await;
    assert_eq!(refused, read(unknown).await);
    let problem: serde_json::Value = serde_json::from_str(&refused.2).expect("a problem document");
    assert_eq!(
        problem["detail"], "Not Found: Session not found",
        "{problem}"
    );
}

/// The one `warn` a refused request files, naming the call that refused it and
/// neither the session nor its owner.
fn expect_refused(logs: &LogCapture, session: &str, call: &str) {
    let event = logs.expect_one("nest_rs::mcp", REFUSED);
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("call").as_deref(), Some(call));
    assert!(
        event
            .fields
            .values()
            .all(|value| !value.contains(session) && !value.contains("alice")),
        "the line names the session or its owner: {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn another_principals_post_on_a_session_is_answered_as_an_unknown_session() {
    let app = serving::<GatedModule>(None).await;
    let session = open_session(app.http(), PATH, Some(&bearer("alice"))).await;
    let logs = LogCapture::install();

    assert_answered_as_unknown(
        list(&app, &session, Some("bob")).await,
        list(&app, "no-such-session", Some("bob")).await,
    )
    .await;
    expect_refused(&logs, &session, "has_session");

    let notified = post_message(
        app.http(),
        PATH,
        Some(&session),
        Some(&bearer("bob")),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    notified.assert_status(StatusCode::NOT_FOUND);

    assert!(
        serves(&app, &session, Some("alice")).await,
        "its owner is still served"
    );
}

#[tokio::test]
async fn another_principals_stream_and_resume_are_answered_as_an_unknown_session() {
    let app = serving::<GatedModule>(None).await;
    let session = open_session(app.http(), PATH, Some(&bearer("alice"))).await;
    let logs = LogCapture::install();

    assert_answered_as_unknown(
        stream(&app, &session, "bob", None).await,
        stream(&app, "no-such-session", "bob", None).await,
    )
    .await;
    assert_answered_as_unknown(
        stream(&app, &session, "bob", Some("0")).await,
        stream(&app, "no-such-session", "bob", Some("0")).await,
    )
    .await;
    assert_eq!(logs.find("nest_rs::mcp", REFUSED).len(), 2);

    stream(&app, &session, "alice", None)
        .await
        .assert_status_is_ok();
    stream(&app, &session, "alice", Some("0"))
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn another_principals_delete_closes_nothing() {
    let app = serving::<GatedModule>(None).await;
    let session = open_session(app.http(), PATH, Some(&bearer("alice"))).await;
    let logs = LogCapture::install();

    assert_answered_as_unknown(
        delete(&app, &session, "bob").await,
        delete(&app, "no-such-session", "bob").await,
    )
    .await;
    expect_refused(&logs, &session, "close_session");
    assert!(
        serves(&app, &session, Some("alice")).await,
        "the session stays open"
    );

    delete(&app, &session, "alice")
        .await
        .assert_status(StatusCode::ACCEPTED);
    list(&app, &session, Some("alice"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// A public endpoint runs no guard at its request, so it authenticates no
/// one: each of its sessions is bound to the anonymous principal, and serves
/// whoever names it, a bearer it never reads included.
#[tokio::test]
async fn a_public_endpoints_sessions_are_the_anonymous_principals() {
    let app = serving::<PublicModule>(None).await;

    let anonymous = open_session(app.http(), PATH, None).await;
    assert!(serves(&app, &anonymous, None).await);
    assert!(serves(&app, &anonymous, Some("alice")).await);

    let bearing = open_session(app.http(), PATH, Some(&bearer("alice"))).await;
    assert!(serves(&app, &bearing, None).await);
}

/// The tenant the principal acts within is part of who it is: the same actor
/// id in another tenant, or a principal naming no actor id in another
/// tenant, is another principal.
#[tokio::test]
async fn a_principal_in_another_tenant_is_another_principal() {
    let app = serving::<GatedModule>(None).await;

    let acme = open_session(app.http(), PATH, Some(&bearer("alice@acme"))).await;
    list(&app, &acme, Some("alice@globex"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    list(&app, &acme, Some("alice"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    assert!(serves(&app, &acme, Some("alice@acme")).await);

    let machine = open_session(app.http(), PATH, Some(&bearer("-@acme"))).await;
    list(&app, &machine, Some("-@globex"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
    assert!(serves(&app, &machine, Some("-@acme")).await);
}

/// A session restored on another instance keeps the principal that opened it.
#[tokio::test]
async fn a_restored_session_keeps_its_principal() {
    let store = Arc::new(MemoryStore::default());
    let opened_here = serving::<GatedModule>(Some(&store)).await;
    let session = open_session(opened_here.http(), PATH, Some(&bearer("alice"))).await;

    let elsewhere = serving::<GatedModule>(Some(&store)).await;
    let logs = LogCapture::install();
    assert_answered_as_unknown(
        list(&elsewhere, &session, Some("bob")).await,
        list(&elsewhere, "no-such-session", Some("bob")).await,
    )
    .await;
    expect_refused(&logs, &session, "has_session");

    let read = store.loads();
    assert!(
        serves(&elsewhere, &session, Some("alice")).await,
        "its owner is served where it was restored"
    );
    assert_eq!(store.loads() - read, 1, "its restore reads the store once");
    list(&elsewhere, &session, Some("bob"))
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

/// The app's store, whose next load waits until the test lets it answer.
struct HeldStore {
    inner: MemoryStore,
    hold_next: AtomicBool,
    held: Notify,
    release: Semaphore,
}

impl HeldStore {
    fn new() -> Self {
        Self {
            inner: MemoryStore::default(),
            hold_next: AtomicBool::new(false),
            held: Notify::new(),
            release: Semaphore::new(0),
        }
    }
}

#[nest_rs_http::async_trait]
impl SessionStore for HeldStore {
    async fn load(&self, session: &str) -> Result<Option<SessionState>, SessionStoreError> {
        if self.hold_next.swap(false, Ordering::SeqCst) {
            self.held.notify_one();
            let _released = self.release.acquire().await.expect("never closed");
        }
        self.inner.load(session).await
    }

    async fn store(&self, session: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        self.inner.store(session, state).await
    }

    async fn delete(&self, session: &str) -> Result<(), SessionStoreError> {
        self.inner.delete(session).await
    }
}

/// rmcp restores one session for one request at a time, every other request
/// naming it waiting on that restore and reading its outcome. Another
/// principal's request held on the store never stands in front of the
/// owner's.
#[tokio::test]
async fn the_owner_is_served_while_another_principals_request_is_held_on_the_store() {
    let store = Arc::new(HeldStore::new());
    let with_store = || {
        TestApp::builder()
            .module::<GatedModule>()
            .provide_dyn::<dyn SessionStore>(Arc::clone(&store) as Arc<dyn SessionStore>)
            .use_guards_global([guard::<ByNameAuthn>()])
            .build()
    };
    let opened_here = with_store().await.expect("boots");
    let session = open_session(opened_here.http(), PATH, Some(&bearer("alice"))).await;
    let elsewhere = with_store().await.expect("boots");

    store.hold_next.store(true, Ordering::SeqCst);
    let bob = list(&elsewhere, &session, Some("bob"));
    let alice = async {
        store.held.notified().await;
        let alice = list(&elsewhere, &session, Some("alice"));
        // Bob's load answers once alice's request is well under way.
        let release = async {
            for _ in 0..200 {
                tokio::task::yield_now().await;
            }
            store.release.add_permits(1);
        };
        tokio::join!(alice, release).0
    };
    let (bob, alice) = tokio::join!(bob, alice);

    alice.assert_status_is_ok();
    bob.assert_status(StatusCode::NOT_FOUND);
}

/// A `DELETE` on an instance not holding the session reaches the app's store,
/// which deletes it for the principal that opened it alone.
#[tokio::test]
async fn a_delete_on_another_instance_deletes_the_owners_session_alone() {
    let store = Arc::new(MemoryStore::default());
    let opened_here = serving::<GatedModule>(Some(&store)).await;
    let session = open_session(opened_here.http(), PATH, Some(&bearer("alice"))).await;
    let elsewhere = serving::<GatedModule>(Some(&store)).await;

    assert_answered_as_unknown(
        delete(&elsewhere, &session, "bob").await,
        delete(&elsewhere, "no-such-session", "bob").await,
    )
    .await;
    assert!(store.holds(&session), "another principal deletes nothing");

    delete(&elsewhere, &session, "alice")
        .await
        .assert_status(StatusCode::ACCEPTED);
    assert!(!store.holds(&session), "its owner deletes it");
}

/// A session stored without the principal that opened it — by a release
/// before the binding — is refused to every caller, never adopted by the
/// first one to name it.
#[tokio::test]
async fn a_session_stored_without_its_principal_is_refused_to_every_caller() {
    let store = Arc::new(MemoryStore::default());
    store.put_unbound("stored-before");
    let app = serving::<GatedModule>(Some(&store)).await;
    let logs = LogCapture::install();

    list(&app, "stored-before", Some("alice"))
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let event = logs.expect_one(
        "nest_rs::mcp",
        "mcp session refused: stored without its principal",
    );
    assert_eq!(event.level, "warn");
    assert!(
        event
            .fields
            .values()
            .all(|value| !value.contains("stored-before")),
        "the line names the session: {:?}",
        event.fields,
    );
    logs.expect_none("nest_rs::mcp", REFUSED);
}
