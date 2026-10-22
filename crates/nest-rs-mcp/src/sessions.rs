//! [`BoundedSessions`] — rmcp's in-memory sessions, held to a maximum and each
//! bound to the principal that opened it — and [`ReportedStore`], the app's
//! session store, keeping that principal beside each session. Each says the
//! failures it hands rmcp, whose own lines are clamped.
//!
//! A session is bound only to what is unique to its principal, as MCP's
//! security guidance asks: on a gated mount the subject its credential names,
//! so a principal naming none is refused a session; on a public mount no guard
//! runs, every caller is anonymous and the session id is a session's only key
//! (`.claude/decisions/mcp-session-bound-to-its-principal.md`).

use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, PoisonError, RwLock};

use futures_util::Stream;
use nest_rs_core::{Correlation, Posture, current_tenant_id, error_message};
use rmcp::model::{ClientJsonRpcMessage, InitializeRequestParams, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_server::session::local::{
    LocalSessionManager, LocalSessionManagerError, SessionConfig,
};
use rmcp::transport::streamable_http_server::session::store::{
    SessionState, SessionStore, SessionStoreError,
};
use rmcp::transport::streamable_http_server::session::{
    EventStore, RestoreOutcome, ServerSseMessage, SessionId, SessionManager,
};
use tokio::sync::Mutex;

use crate::error::SessionsError;

tokio::task_local! {
    /// The request rmcp is answering while [`BoundedSessions::answering`]
    /// runs its answer: rmcp creates, restores and reaches the request's
    /// session on the task answering it.
    static ANSWERING: Answering;
}

/// The request rmcp is answering: who it is served for, why a manager refused
/// it, which the endpoint answers in place of rmcp, and what the app's store
/// held for its session, read for the restore that follows.
struct Answering {
    caller: Principal,
    refused: Cell<Option<Refused>>,
    stored: Cell<Option<Stored>>,
}

/// What the app's store held for a session, the caller's or none, when the
/// request being answered asked whose it is.
struct Stored {
    id: SessionId,
    state: Option<SessionState>,
}

/// Why a manager refused the request rmcp is answering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The mount holds its maximum: rmcp answers `500`, the endpoint the
    /// cap's `503`.
    Full,
    /// The session is another principal's, or one the mount does not hold:
    /// answered as rmcp answers a session it does not hold.
    Unknown,
    /// The caller's principal names no subject a session could be bound to:
    /// the endpoint answers why, and opens none.
    NoSubject,
}

/// Who a request is served for, as a session is bound to it: the actor id and
/// the tenant the authentication guard read off the verified principal, and
/// neither for an anonymous caller.
///
/// MCP's security guidance binds a session to "information unique to the
/// authorized user": a request on a session another principal opened is
/// answered as one on a session the mount does not hold. The tenant alone is
/// not unique to a principal, so on a gated mount only one naming its subject
/// is bound a session ([`binds_on`](Self::binds_on)).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Principal {
    actor: Option<String>,
    tenant: Option<String>,
}

/// The principal outside any request: anonymous.
const ANONYMOUS: Principal = Principal {
    actor: None,
    tenant: None,
};

impl Principal {
    /// The principal `request` is served for, read past its guards: the actor
    /// the authentication guard recorded, and the tenant it made ambient.
    pub(crate) fn of(request: &Correlation) -> Self {
        Self {
            actor: request.actor_id().map(str::to_owned),
            tenant: current_tenant_id(),
        }
    }

    /// Whether a session may be bound to this principal on a mount of
    /// `posture`: on a public one every caller is anonymous and the session
    /// id is the key; on any other, only a principal naming its subject.
    pub(crate) fn binds_on(&self, posture: Posture) -> bool {
        posture == Posture::Public || self.actor.is_some()
    }
}

/// The principal the request being answered is served for; anonymous
/// outside one.
fn caller() -> Principal {
    ANSWERING
        .try_with(|answering| answering.caller.clone())
        .unwrap_or(ANONYMOUS)
}

/// Whether `owner` is the principal the request being answered is served for.
fn is_caller(owner: &Principal) -> bool {
    ANSWERING
        .try_with(|answering| answering.caller == *owner)
        .unwrap_or(*owner == ANONYMOUS)
}

/// Whether a manager refused the request being answered already.
fn refused() -> bool {
    ANSWERING
        .try_with(|answering| answering.refused.get().is_some())
        .unwrap_or_default()
}

/// Mark the request being answered refused for `why`; the first reason
/// stands. Outside `answering`, no answer reads the mark.
fn refuse(why: Refused) {
    ANSWERING
        .try_with(|answering| {
            if answering.refused.get().is_none() {
                answering.refused.set(Some(why));
            }
        })
        .unwrap_or_default();
}

/// Refuse the request being answered a session another principal opened,
/// said once at `warn` by the `call` that refused it, never naming the session
/// or its owner.
fn refuse_foreign(call: &'static str) {
    if !refused() {
        tracing::warn!(
            target: crate::TARGET,
            call,
            "mcp session refused: another principal opened it",
        );
    }
    refuse(Refused::Unknown);
}

/// [`SessionsError::Unknown`], the request being answered refused at `call`
/// a session another principal opened.
fn foreign(call: &'static str) -> SessionsError {
    refuse_foreign(call);
    SessionsError::Unknown
}

/// What a refusal for a principal naming no subject tells the operator.
const NO_SUBJECT_HINT: &str = "return the verified credential's subject from \
                               PrincipalIdentity::actor_id (RFC 9068 §2.2 requires `sub`); a \
                               client with none calls statelessly, on protocol 2026-07-28";

/// Say a session refused because its principal names no subject: by the
/// check in front of rmcp, or by the manager past it; the client reads one
/// `403` either way, and the line names neither the credential nor the
/// tenant.
pub(crate) fn say_no_subject() {
    tracing::warn!(
        target: crate::TARGET,
        hint = NO_SUBJECT_HINT,
        "mcp session refused: its principal names no subject",
    );
}

/// Keep `state`, what the app's store holds for session `id`, on the request
/// being answered, for its restore to take.
fn keep_stored(id: &SessionId, state: Option<SessionState>) {
    ANSWERING
        .try_with(|answering| {
            answering.stored.set(Some(Stored {
                id: id.clone(),
                state,
            }));
        })
        .unwrap_or_default();
}

/// What the request being answered read of session `id` in the app's store,
/// taken; `None` when it read nothing of it.
fn take_stored(id: &str) -> Option<Option<SessionState>> {
    ANSWERING
        .try_with(|answering| answering.stored.take())
        .ok()
        .flatten()
        .filter(|stored| &*stored.id == id)
        .map(|stored| stored.state)
}

/// rmcp's local session manager, refusing a session past `max` with
/// [`SessionsError::Full`] and serving each session to the principal that
/// opened it alone.
pub(crate) struct BoundedSessions {
    inner: LocalSessionManager,
    max: usize,
    /// Held across the count and the insert, so the cap is exact: a close only
    /// lowers the count.
    creating: Mutex<()>,
    /// The principal each live session is bound to: the one its `initialize`,
    /// or the request restoring it, was served for.
    owners: RwLock<HashMap<SessionId, Principal>>,
    /// Who the mount serves, which decides what a session is bound to.
    posture: Posture,
    /// The app's store, holding the sessions other instances opened.
    store: Option<Arc<ReportedStore>>,
}

impl BoundedSessions {
    /// rmcp's local manager running each session on `session`, holding at
    /// most `max` live sessions on a mount of `posture`, and asking `store`
    /// whose a session it does not hold is.
    pub(crate) fn new(
        session: SessionConfig,
        max: usize,
        posture: Posture,
        store: Option<Arc<ReportedStore>>,
    ) -> Self {
        // `#[non_exhaustive]`: rmcp's default, its session config replaced.
        let mut inner = LocalSessionManager::default();
        inner.session_config = session;
        Self {
            inner,
            max,
            creating: Mutex::new(()),
            owners: RwLock::new(HashMap::new()),
            posture,
            store,
        }
    }

    /// Whether this mount may bind a session to `caller`.
    pub(crate) fn binds(&self, caller: &Principal) -> bool {
        caller.binds_on(self.posture)
    }

    /// Whether a new session would be refused now.
    pub(crate) async fn is_full(&self) -> bool {
        self.inner.sessions.read().await.len() >= self.max
    }

    /// Say a session refused at the cap: by the check in front of rmcp, or by
    /// this manager past it; the client reads one `503` either way.
    pub(crate) fn say_full(&self) {
        tracing::warn!(
            target: crate::TARGET,
            max_concurrent_sessions = self.max,
            "mcp session refused: the mount holds its maximum",
        );
    }

    /// `SessionsError::Full`, said, and marked on the request being answered.
    fn full(&self) -> SessionsError {
        self.say_full();
        refuse(Refused::Full);
        SessionsError::Full
    }

    /// `SessionsError::NoSubject`, said, and marked on the request being
    /// answered.
    fn no_subject() -> SessionsError {
        say_no_subject();
        refuse(Refused::NoSubject);
        SessionsError::NoSubject
    }

    /// `answering`, the answer to one request served for `caller`, and why a
    /// manager refused it while it ran.
    pub(crate) async fn answering<F: Future>(
        caller: Principal,
        answering: F,
    ) -> (F::Output, Option<Refused>) {
        let request = Answering {
            caller,
            refused: Cell::new(None),
            stored: Cell::new(None),
        };
        ANSWERING
            .scope(request, async {
                let answer = answering.await;
                (answer, ANSWERING.with(|answering| answering.refused.get()))
            })
            .await
    }

    /// Whether session `id` is bound to the caller, to another principal, or
    /// to none this manager knows of.
    fn bound_to_caller(&self, id: &SessionId) -> Option<bool> {
        self.owners
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .map(is_caller)
    }

    /// Bind session `id` to `owner`, the principal the request being answered
    /// is served for.
    fn bind(&self, id: SessionId, owner: Principal) {
        self.owners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, owner);
    }

    /// Refuse, at `call`, the request being answered a session another
    /// principal opened; one this manager does not hold is rmcp's to refuse.
    fn admit(&self, id: &SessionId, call: &'static str) -> Result<(), SessionsError> {
        match self.bound_to_caller(id) {
            Some(false) => Err(foreign(call)),
            Some(true) | None => Ok(()),
        }
    }

    /// Whether a client's `DELETE` may close session `id`: one bound to its
    /// principal, here or in the app's store. Any other is refused as
    /// unknown, which rmcp would answer `202`, so a `DELETE` never says
    /// whether another principal's session exists.
    async fn admit_close(&self, id: &SessionId) -> Result<(), SessionsError> {
        let mut bound = self.bound_to_caller(id);
        // Held elsewhere: rmcp deletes it from the app's store next.
        if bound.is_none()
            && let Some(store) = &self.store
        {
            bound = store
                .bound(id, "close_session")
                .await
                .map_err(SessionsError::Store)?
                .map(|(owner, _)| is_caller(&owner));
        }
        match bound {
            Some(true) => Ok(()),
            Some(false) => Err(foreign("close_session")),
            None => {
                refuse(Refused::Unknown);
                Err(SessionsError::Unknown)
            }
        }
    }
}

/// `outcome`, its failure said at `error`: rmcp answers it `500`, or nothing
/// at all for a session its worker closes.
fn said<T>(
    call: &'static str,
    outcome: Result<T, LocalSessionManagerError>,
) -> Result<T, SessionsError> {
    outcome.map_err(|error| {
        tracing::error!(
            target: crate::TARGET,
            call,
            error = %error_message(&error),
            "mcp session manager failed",
        );
        SessionsError::from(error)
    })
}

impl SessionManager for BoundedSessions {
    type Error = SessionsError;
    type Transport = <LocalSessionManager as SessionManager>::Transport;

    /// A session opens only for a principal it can be bound to, and under the
    /// cap.
    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        let owner = caller();
        if !self.binds(&owner) {
            return Err(Self::no_subject());
        }
        let _creating = self.creating.lock().await;
        if self.is_full().await {
            return Err(self.full());
        }
        let (id, transport) = said("create_session", self.inner.create_session().await)?;
        self.bind(id.clone(), owner);
        Ok((id, transport))
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        self.admit(id, "initialize_session")?;
        said(
            "initialize_session",
            self.inner.initialize_session(id, message).await,
        )
    }

    /// Whether the caller's session `id` is here; `false` sends rmcp to the
    /// app's store. Another principal's, here or in the store, is an error,
    /// which rmcp answers at once: on `false` it registers a restore, and
    /// every request naming the session waits on that restore and reads its
    /// refusal as its own, the owner's included.
    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        if said("has_session", self.inner.has_session(id).await)? {
            return match self.bound_to_caller(id) {
                Some(true) => Ok(true),
                Some(false) => Err(foreign("has_session")),
                // Restored, not bound yet: the caller waits on that restore.
                None => Ok(false),
            };
        }
        let Some(store) = &self.store else {
            return Ok(false);
        };
        match store
            .bound(id, "has_session")
            .await
            .map_err(SessionsError::Store)?
        {
            Some((owner, _)) if !is_caller(&owner) => Err(foreign("has_session")),
            stored => {
                keep_stored(id, stored.map(|(_, state)| state));
                Ok(false)
            }
        }
    }

    /// A client's `DELETE` closes its own session alone; rmcp closing a
    /// session its worker ended runs outside any request.
    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        if ANSWERING.try_with(|_| ()).is_ok() {
            self.admit_close(id).await?;
        }
        let closed = said("close_session", self.inner.close_session(id).await);
        self.owners
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        closed
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.admit(id, "create_stream")?;
        said("create_stream", self.inner.create_stream(id, message).await)
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        self.admit(id, "accept_message")?;
        said(
            "accept_message",
            self.inner.accept_message(id, message).await,
        )
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.admit(id, "create_standalone_stream")?;
        said(
            "create_standalone_stream",
            self.inner.create_standalone_stream(id).await,
        )
    }

    /// rmcp answers a resume it cannot serve with an empty stream, so its
    /// failure — most often a `Last-Event-ID` the session no longer holds — is
    /// said at `warn`.
    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        self.admit(id, "resume")?;
        self.inner.resume(id, last_event_id).await.map_err(|error| {
            tracing::warn!(
                target: crate::TARGET,
                error = %error_message(&error),
                "mcp stream not resumed: answered with an empty stream",
            );
            SessionsError::from(error)
        })
    }

    /// A restored session takes a place like a created one; one already
    /// present takes none. rmcp restores only what the store loaded for the
    /// request being answered ([`ReportedStore::load`]), so a restored
    /// session is bound to that request's principal.
    async fn restore_session(
        &self,
        id: SessionId,
    ) -> Result<RestoreOutcome<Self::Transport>, Self::Error> {
        let _creating = self.creating.lock().await;
        if !said("has_session", self.inner.has_session(&id).await)? && self.is_full().await {
            return Err(self.full());
        }
        let restored = said(
            "restore_session",
            self.inner.restore_session(id.clone()).await,
        )?;
        if matches!(restored, RestoreOutcome::Restored(_)) {
            self.bind(id, caller());
        }
        Ok(restored)
    }

    fn event_store(&self) -> Option<Arc<dyn EventStore>> {
        self.inner.event_store()
    }
}

/// The `_meta` key of a stored session's handshake under which the mount
/// keeps the principal the session is bound to. MCP's `_meta` keys take a
/// reverse-DNS prefix, here the project's domain.
const PRINCIPAL_KEY: &str = "dev.nestrs/principal";

/// `params` carrying `owner` under [`PRINCIPAL_KEY`], in place of whatever
/// the client wrote there.
fn put_principal(params: &mut InitializeRequestParams, owner: &Principal) {
    params
        .meta
        .get_or_insert_with(Default::default)
        .insert(PRINCIPAL_KEY.to_owned(), serde_json::json!(owner));
}

/// The principal `params` carry under [`PRINCIPAL_KEY`], taken out so the
/// handshake a restore replays is the client's own; `None` when they carry
/// none it reads.
fn take_principal(params: &mut InitializeRequestParams) -> Option<Principal> {
    let meta = params.meta.as_mut()?;
    let owner = meta.remove(PRINCIPAL_KEY);
    if meta.is_empty() {
        params.meta = None;
    }
    serde_json::from_value(owner?).ok()
}

/// The app's `SessionStore`, keeping beside each session the principal it is
/// bound to and each failure said: rmcp's own lines name the session.
pub(crate) struct ReportedStore {
    inner: Arc<dyn SessionStore>,
    /// Who the mount serves, which decides what a stored binding may hold.
    posture: Posture,
}

impl ReportedStore {
    /// `inner`, the app's store, behind a mount of `posture`.
    pub(crate) fn new(inner: Arc<dyn SessionStore>, posture: Posture) -> Self {
        Self { inner, posture }
    }

    /// The state the store holds for session `id`, said when the load fails.
    async fn loaded(&self, id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        self.inner.load(id).await.inspect_err(|error| {
            tracing::error!(
                target: crate::TARGET,
                call = "load",
                error = %error_message(error.as_ref()),
                "mcp session store failed",
            );
        })
    }

    /// The principal session `id` is bound to and its state as the client
    /// sent it, when the store holds it with one this mount binds. Read as
    /// none, said at `warn` by the `call` reading it, so no caller adopts it:
    /// a session stored without one, by a release before the binding, and on
    /// a gated mount one bound to a principal naming no subject — every
    /// session a public mount sharing the store opened, bound to the
    /// anonymous principal.
    async fn bound(
        &self,
        id: &str,
        call: &'static str,
    ) -> Result<Option<(Principal, SessionState)>, SessionStoreError> {
        let Some(mut state) = self.loaded(id).await? else {
            return Ok(None);
        };
        let Some(owner) = take_principal(&mut state.initialize_params) else {
            tracing::warn!(
                target: crate::TARGET,
                call,
                "mcp session refused: stored without its principal",
            );
            return Ok(None);
        };
        if !owner.binds_on(self.posture) {
            tracing::warn!(
                target: crate::TARGET,
                call,
                "mcp session refused: stored for a principal naming no subject",
            );
            return Ok(None);
        }
        Ok(Some((owner, state)))
    }
}

#[async_trait::async_trait]
impl SessionStore for ReportedStore {
    /// rmcp answers a failed load `500`, and a session it is not handed
    /// `404`: one bound to another principal is not handed. What
    /// [`BoundedSessions::has_session`] read for the request is handed as
    /// read, its owner checked there.
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        if let Some(read) = take_stored(session_id) {
            return Ok(read);
        }
        let Some((owner, state)) = self.bound(session_id, "load").await? else {
            return Ok(None);
        };
        if !is_caller(&owner) {
            refuse_foreign("load");
            return Ok(None);
        }
        Ok(Some(state))
    }

    /// rmcp serves a session it failed to store, which another instance then
    /// cannot restore.
    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        let mut bound = state.clone();
        put_principal(&mut bound.initialize_params, &caller());
        self.inner
            .store(session_id, &bound)
            .await
            .inspect_err(|error| {
                tracing::warn!(
                    target: crate::TARGET,
                    call = "store",
                    error = %error_message(error.as_ref()),
                    "mcp session store failed",
                );
            })
    }

    /// rmcp closes the session all the same, its stored state left behind.
    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError> {
        self.inner.delete(session_id).await.inspect_err(|error| {
            tracing::warn!(
                target: crate::TARGET,
                call = "delete",
                error = %error_message(error.as_ref()),
                "mcp session store failed",
            );
        })
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_testing::{CapturedEvent, LogCapture};

    use super::*;

    /// One place, on a public mount: every caller here is anonymous.
    fn one_place() -> BoundedSessions {
        BoundedSessions::new(SessionConfig::default(), 1, Posture::Public, None)
    }

    /// Two places on a gated mount, holding no store.
    fn gated() -> BoundedSessions {
        BoundedSessions::new(SessionConfig::default(), 2, Posture::Gated, None)
    }

    /// The session id a client names, which no line may carry.
    const NAMED: &str = "named-by-a-client";

    fn named() -> SessionId {
        SessionId::from(NAMED)
    }

    fn ping() -> ClientJsonRpcMessage {
        serde_json::from_value(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
            .expect("a ping")
    }

    /// `event` leaves out the session a client named.
    fn without_the_session(event: &CapturedEvent) {
        assert!(
            event.fields.values().all(|value| !value.contains(NAMED)),
            "the session a client named is quoted: {:?}",
            event.fields
        );
    }

    /// rmcp's own line for each is clamped: the manager says what it hands
    /// rmcp to answer `500`, by kind.
    #[tokio::test]
    async fn every_failure_handed_to_rmcp_is_said_by_its_kind() {
        let sessions = one_place();
        let logs = LogCapture::install();

        assert!(sessions.initialize_session(&named(), ping()).await.is_err());
        assert!(sessions.create_stream(&named(), ping()).await.is_err());
        assert!(sessions.accept_message(&named(), ping()).await.is_err());
        assert!(sessions.create_standalone_stream(&named()).await.is_err());

        let said = logs.find(crate::TARGET, "mcp session manager failed");
        let calls: Vec<_> = said
            .iter()
            .filter_map(|event| event.field("call"))
            .collect();
        assert_eq!(
            calls,
            [
                "initialize_session",
                "create_stream",
                "accept_message",
                "create_standalone_stream"
            ],
        );
        for event in &said {
            assert_eq!(event.level, "error");
            assert_eq!(event.field("error").as_deref(), Some("session not found"));
            without_the_session(event);
        }
    }

    /// rmcp answers a resume it cannot serve with an empty stream.
    #[tokio::test]
    async fn a_resume_it_cannot_serve_is_said_at_warn() {
        let sessions = one_place();
        let logs = LogCapture::install();

        assert!(sessions.resume(&named(), "0".to_owned()).await.is_err());

        let event = logs.expect_one(
            crate::TARGET,
            "mcp stream not resumed: answered with an empty stream",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("error").as_deref(), Some("session not found"));
        without_the_session(&event);
    }

    /// One line, the cap's, whichever check refused.
    #[tokio::test]
    async fn a_session_refused_past_the_cap_is_said_as_the_cap_says_it() {
        let sessions = one_place();
        sessions
            .create_session()
            .await
            .expect("the first session takes the one place");
        let logs = LogCapture::install();

        assert!(sessions.create_session().await.is_err());
        assert!(sessions.restore_session(named()).await.is_err());

        let said = logs.find(
            crate::TARGET,
            "mcp session refused: the mount holds its maximum",
        );
        assert_eq!(said.len(), 2, "{said:?}");
        for event in &said {
            assert_eq!(event.level, "warn");
            assert_eq!(event.field("max_concurrent_sessions").as_deref(), Some("1"));
        }
        logs.expect_none(crate::TARGET, "mcp session manager failed");
    }

    /// A store whose every call fails, in its own words.
    struct Down;

    const DOWN: &str = "the store is down";

    #[async_trait::async_trait]
    impl SessionStore for Down {
        async fn load(&self, _: &str) -> Result<Option<SessionState>, SessionStoreError> {
            Err(DOWN.into())
        }

        async fn store(&self, _: &str, _: &SessionState) -> Result<(), SessionStoreError> {
            Err(DOWN.into())
        }

        async fn delete(&self, _: &str) -> Result<(), SessionStoreError> {
            Err(DOWN.into())
        }
    }

    /// rmcp's own lines name the session; the store's failures are said
    /// without it, at the level of what rmcp does next.
    #[tokio::test]
    async fn a_store_failure_is_said_at_the_level_of_what_rmcp_does_next() {
        let store = ReportedStore::new(Arc::new(Down), Posture::Gated);
        let state: SessionState = serde_json::from_value(serde_json::json!({
            "initialize_params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "client", "version": "0" },
            },
        }))
        .expect("a session's state");
        let logs = LogCapture::install();

        assert!(store.load(NAMED).await.is_err());
        assert!(store.store(NAMED, &state).await.is_err());
        assert!(store.delete(NAMED).await.is_err());

        let said = logs.find(crate::TARGET, "mcp session store failed");
        let levels: Vec<_> = said
            .iter()
            .map(|event| {
                (
                    event.field("call").unwrap_or_default(),
                    event.level.as_str(),
                )
            })
            .collect();
        assert_eq!(
            levels,
            [
                ("load".to_owned(), "error"),
                ("store".to_owned(), "warn"),
                ("delete".to_owned(), "warn")
            ],
        );
        for event in &said {
            assert_eq!(event.field("error").as_deref(), Some(DOWN));
            without_the_session(event);
        }
    }

    #[tokio::test]
    async fn a_session_past_the_cap_is_refused_by_the_manager_itself() {
        let sessions = one_place();
        sessions
            .create_session()
            .await
            .expect("the first session takes the one place");

        assert!(
            matches!(sessions.create_session().await, Err(SessionsError::Full)),
            "the manager refuses past the cap even when no pre-check ran",
        );
    }

    /// Two creations racing for the last place on two threads: the count and
    /// the insert are one step, so exactly one is let in.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_creations_racing_for_the_last_place_let_exactly_one_in() {
        let sessions = Arc::new(one_place());
        let start = Arc::new(tokio::sync::Barrier::new(2));
        let racer = || {
            let sessions = Arc::clone(&sessions);
            let start = Arc::clone(&start);
            tokio::spawn(async move {
                start.wait().await;
                sessions.create_session().await.map(|_| ())
            })
        };
        let (first, second) = tokio::join!(racer(), racer());

        let outcomes = [
            first.expect("the first racer ran"),
            second.expect("the second racer ran"),
        ];
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert!(
            outcomes
                .iter()
                .any(|outcome| matches!(outcome, Err(SessionsError::Full))),
            "the other is refused at the cap",
        );
    }

    #[tokio::test]
    async fn a_restored_session_takes_a_place_like_a_created_one() {
        let sessions = one_place();
        let restored = sessions
            .restore_session(SessionId::from("stored"))
            .await
            .expect("a restore under the cap is let in");
        assert!(matches!(restored, RestoreOutcome::Restored(_)));

        assert!(
            matches!(sessions.create_session().await, Err(SessionsError::Full)),
            "the restored session holds the one place",
        );
    }

    #[tokio::test]
    async fn a_restore_of_an_unknown_session_past_the_cap_is_refused() {
        let sessions = one_place();
        sessions
            .create_session()
            .await
            .expect("the first session takes the one place");

        assert!(matches!(
            sessions.restore_session(SessionId::from("stored")).await,
            Err(SessionsError::Full),
        ));
    }

    #[tokio::test]
    async fn a_restore_of_a_session_already_present_takes_no_place() {
        let sessions = one_place();
        let (id, _transport) = sessions
            .create_session()
            .await
            .expect("the first session takes the one place");

        assert!(
            matches!(
                sessions.restore_session(id).await,
                Ok(RestoreOutcome::AlreadyPresent),
            ),
            "a session in memory is answered present, not refused, at the cap",
        );
    }

    #[tokio::test]
    async fn a_closed_session_frees_its_place() {
        let sessions = one_place();
        let (id, _transport) = sessions
            .create_session()
            .await
            .expect("the first session takes the one place");
        sessions
            .close_session(&id)
            .await
            .expect("the session closes");

        sessions
            .create_session()
            .await
            .expect("the freed place takes a new session");
    }

    fn principal(actor: Option<&str>, tenant: Option<&str>) -> Principal {
        Principal {
            actor: actor.map(str::to_owned),
            tenant: tenant.map(str::to_owned),
        }
    }

    fn named_actor(actor: &str) -> Principal {
        principal(Some(actor), None)
    }

    /// A session `owner` opened on `sessions`.
    async fn opened_by(sessions: &BoundedSessions, owner: Principal) -> SessionId {
        let ((id, _transport), refused) = BoundedSessions::answering(owner, async {
            sessions.create_session().await.expect("a session opens")
        })
        .await;
        assert_eq!(refused, None);
        id
    }

    /// rmcp asks `has_session` first, but a request waiting on another's
    /// restore of the same session skips it: every call that reaches a
    /// session refuses another principal's request itself, and the request
    /// files one line.
    #[tokio::test]
    async fn every_call_reaching_another_principals_session_refuses_it() {
        let sessions = gated();
        let id = opened_by(&sessions, named_actor("alice")).await;
        let logs = LogCapture::install();

        let (refusals, refused) = BoundedSessions::answering(named_actor("bob"), async {
            [
                sessions.initialize_session(&id, ping()).await.err(),
                sessions.create_stream(&id, ping()).await.err(),
                sessions.accept_message(&id, ping()).await.err(),
                sessions.create_standalone_stream(&id).await.err(),
                sessions.resume(&id, "0".to_owned()).await.err(),
                sessions.close_session(&id).await.err(),
            ]
        })
        .await;

        for refusal in refusals {
            assert!(
                matches!(refusal, Some(SessionsError::Unknown)),
                "{refusal:?}"
            );
        }
        assert_eq!(refused, Some(Refused::Unknown));
        let event = logs.expect_one(
            crate::TARGET,
            "mcp session refused: another principal opened it",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("call").as_deref(), Some("initialize_session"));
        logs.expect_none(
            crate::TARGET,
            "mcp stream not resumed: answered with an empty stream",
        );
        logs.expect_none(crate::TARGET, "mcp session manager failed");

        let (held, refused) = BoundedSessions::answering(named_actor("bob"), async {
            sessions.has_session(&id).await
        })
        .await;
        assert!(
            matches!(held, Err(SessionsError::Unknown)),
            "refused, so rmcp registers no restore of it: {held:?}"
        );
        assert_eq!(refused, Some(Refused::Unknown));

        let (held, refused) = BoundedSessions::answering(named_actor("alice"), async {
            sessions.has_session(&id).await.expect("answered")
        })
        .await;
        assert!(held, "its owner still has it: nothing closed it");
        assert_eq!(refused, None);
    }

    /// A session's worker ending closes it outside any request, whoever
    /// opened it.
    #[tokio::test]
    async fn rmcps_own_close_outside_a_request_closes_any_session() {
        let sessions = gated();
        let id = opened_by(&sessions, named_actor("alice")).await;

        sessions
            .close_session(&id)
            .await
            .expect("rmcp's close is let through");

        let (held, _) = BoundedSessions::answering(named_actor("alice"), async {
            sessions.has_session(&id).await.expect("answered")
        })
        .await;
        assert!(!held);
        assert!(
            sessions
                .owners
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "a closed session leaves no binding behind"
        );
    }

    /// rmcp answers a `DELETE` of a session it does not hold `202`; the
    /// manager refuses it as unknown, as it refuses another principal's.
    #[tokio::test]
    async fn a_delete_of_a_session_the_mount_does_not_hold_is_refused_as_unknown() {
        let sessions = one_place();
        let logs = LogCapture::install();

        let (closed, refused) = BoundedSessions::answering(named_actor("alice"), async {
            sessions.close_session(&named()).await
        })
        .await;

        assert!(matches!(closed, Err(SessionsError::Unknown)), "{closed:?}");
        assert_eq!(refused, Some(Refused::Unknown));
        logs.expect_none(
            crate::TARGET,
            "mcp session refused: another principal opened it",
        );
    }

    /// Only the subject is unique to a principal: the tenant alone is shared
    /// by every machine client of it, and no tenant by every caller naming
    /// none. A public mount binds the anonymous caller, the session id its
    /// only key; any posture but `Public` binds as a gated mount does.
    #[test]
    fn a_session_binds_a_principal_naming_its_subject_or_any_caller_of_a_public_mount() {
        for (owner, gated) in [
            (principal(None, None), false),
            (principal(None, Some("acme")), false),
            (principal(Some("alice"), None), true),
            (principal(Some("alice"), Some("acme")), true),
        ] {
            assert!(owner.binds_on(Posture::Public), "{owner:?}");
            assert_eq!(owner.binds_on(Posture::Gated), gated, "{owner:?}");
            assert_eq!(owner.binds_on(Posture::InBand), gated, "{owner:?}");
        }
    }

    /// A gated mount opens no session for a principal naming no subject,
    /// whichever tenant it acts within: the request is marked for the
    /// endpoint's `403` and files one line, naming neither.
    #[tokio::test]
    async fn a_gated_mount_opens_no_session_for_a_principal_naming_no_subject() {
        let sessions = gated();
        let logs = LogCapture::install();

        for caller in [principal(None, Some("acme")), principal(None, None)] {
            let (created, refused) = BoundedSessions::answering(caller.clone(), async {
                sessions.create_session().await.map(|(id, _)| id)
            })
            .await;
            assert!(
                matches!(created, Err(SessionsError::NoSubject)),
                "{caller:?}: {created:?}"
            );
            assert_eq!(refused, Some(Refused::NoSubject), "{caller:?}");
        }
        assert!(
            sessions.inner.sessions.read().await.is_empty(),
            "no session opened"
        );
        let said = logs.find(
            crate::TARGET,
            "mcp session refused: its principal names no subject",
        );
        assert_eq!(said.len(), 2, "{said:?}");
        for event in &said {
            assert_eq!(event.level, "warn");
            assert!(event.field("hint").is_some(), "{:?}", event.fields);
            assert!(
                event.fields.values().all(|value| !value.contains("acme")),
                "{:?}",
                event.fields
            );
        }

        opened_by(&sessions, principal(Some("alice"), Some("acme"))).await;
        opened_by(&one_place(), ANONYMOUS).await;
    }

    /// The app's store holding one session's state, as a deployment's would,
    /// and refusing to change it.
    struct OneSession(SessionState);

    const READ_ONLY: &str = "the store is read-only";

    #[async_trait::async_trait]
    impl SessionStore for OneSession {
        async fn load(&self, _: &str) -> Result<Option<SessionState>, SessionStoreError> {
            Ok(Some(self.0.clone()))
        }

        async fn store(&self, _: &str, _: &SessionState) -> Result<(), SessionStoreError> {
            Err(READ_ONLY.into())
        }

        async fn delete(&self, _: &str) -> Result<(), SessionStoreError> {
            Err(READ_ONLY.into())
        }
    }

    /// A session bound to a principal naming no subject, as a public mount
    /// binds each of its own to the anonymous principal: a gated mount
    /// reading the same store adopts it for no caller and says so, and a
    /// public mount serves its anonymous sessions alone.
    #[tokio::test]
    async fn a_stored_binding_naming_no_subject_is_adopted_on_a_public_mount_alone() {
        for owner in [principal(None, Some("acme")), ANONYMOUS] {
            let mut params = handshake(None);
            put_principal(&mut params, &owner);
            let held = Arc::new(OneSession(SessionState::new(params)));

            let logs = LogCapture::install();
            let gated = ReportedStore::new(held.clone(), Posture::Gated);
            let (loaded, refused) = BoundedSessions::answering(owner.clone(), async {
                gated.load(NAMED).await.expect("the store answers")
            })
            .await;
            assert!(loaded.is_none(), "{owner:?} adopts it on a gated mount");
            assert_eq!(refused, None, "read as a session the store does not hold");
            let event = logs.expect_one(
                crate::TARGET,
                "mcp session refused: stored for a principal naming no subject",
            );
            assert_eq!(event.level, "warn");
            assert_eq!(event.field("call").as_deref(), Some("load"));
            without_the_session(&event);

            let public = ReportedStore::new(held, Posture::Public);
            let (loaded, _) = BoundedSessions::answering(ANONYMOUS, async {
                public.load(NAMED).await.expect("the store answers")
            })
            .await;
            assert_eq!(
                loaded.is_some(),
                owner == ANONYMOUS,
                "a public mount serves its anonymous sessions alone: {owner:?}"
            );
        }
    }

    /// What `has_session` read of the app's store is handed to the restore of
    /// that session alone, once.
    #[tokio::test]
    async fn a_store_read_is_handed_to_its_sessions_restore_alone_once() {
        let state: SessionState =
            serde_json::from_value(serde_json::json!({ "initialize_params": handshake(None) }))
                .expect("a session's state");

        let (taken, _) = BoundedSessions::answering(ANONYMOUS, async {
            keep_stored(&named(), Some(state.clone()));
            let another = take_stored("another-session");
            keep_stored(&named(), Some(state));
            [another, take_stored(NAMED), take_stored(NAMED)]
        })
        .await;

        assert_eq!(
            taken.map(|read| read.map(|state| state.is_some())),
            [None, Some(true), None]
        );
    }

    fn handshake(meta: Option<serde_json::Value>) -> InitializeRequestParams {
        let mut params = serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "client", "version": "0" },
        });
        if let Some(meta) = meta {
            params["_meta"] = meta;
        }
        serde_json::from_value(params).expect("a handshake")
    }

    /// The principal travels beside the client's handshake, through what a
    /// store serializes, and leaves it as the client sent it.
    #[test]
    fn a_principal_travels_beside_the_handshake_and_leaves_it_as_sent() {
        let acme = Principal {
            actor: None,
            tenant: Some("acme".to_owned()),
        };
        for sent in [
            handshake(None),
            handshake(Some(serde_json::json!({ "progressToken": 7 }))),
        ] {
            let mut stored = sent.clone();
            put_principal(&mut stored, &acme);
            let mut stored: InitializeRequestParams =
                serde_json::from_value(serde_json::to_value(&stored).expect("serializes"))
                    .expect("deserializes");

            assert_eq!(take_principal(&mut stored), Some(acme.clone()));
            assert_eq!(stored, sent);
        }

        let mut written_by_the_client = handshake(Some(
            serde_json::json!({ PRINCIPAL_KEY: { "actor": "alice", "tenant": null } }),
        ));
        put_principal(&mut written_by_the_client, &ANONYMOUS);
        assert_eq!(
            take_principal(&mut written_by_the_client),
            Some(ANONYMOUS),
            "what the mount writes replaces what the client wrote"
        );
    }

    #[test]
    fn a_handshake_carrying_no_principal_it_reads_names_none() {
        assert_eq!(take_principal(&mut handshake(None)), None);
        for unread in [
            serde_json::json!({ "progressToken": 7 }),
            serde_json::json!({ PRINCIPAL_KEY: "alice" }),
            serde_json::json!({ PRINCIPAL_KEY: { "actor": "alice", "role": "admin" } }),
        ] {
            assert_eq!(
                take_principal(&mut handshake(Some(unread.clone()))),
                None,
                "{unread}"
            );
        }
    }
}
