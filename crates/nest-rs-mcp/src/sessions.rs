//! [`BoundedSessions`] — rmcp's in-memory sessions, held to a maximum — and
//! [`ReportedStore`], the app's session store. Each says the failures it hands
//! rmcp, whose own lines are clamped.

use std::cell::Cell;
use std::future::Future;
use std::sync::Arc;

use futures_util::Stream;
use nest_rs_core::error_message;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
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
    /// Set when a manager refuses a session at the cap while
    /// [`BoundedSessions::watching`] runs a request's answer: rmcp turns the
    /// refusal into a `500`, which the endpoint answers as the cap's `503`.
    static REFUSED_AT_CAP: Cell<bool>;
}

/// rmcp's local session manager, refusing a session past `max` with
/// [`SessionsError::Full`].
pub(crate) struct BoundedSessions {
    inner: LocalSessionManager,
    max: usize,
    /// Held across the count and the insert, so the cap is exact: a close only
    /// lowers the count.
    creating: Mutex<()>,
}

impl BoundedSessions {
    /// rmcp's local manager running each session on `session`, holding at
    /// most `max` live sessions.
    pub(crate) fn new(session: SessionConfig, max: usize) -> Self {
        // `#[non_exhaustive]`: rmcp's default, its session config replaced.
        let mut inner = LocalSessionManager::default();
        inner.session_config = session;
        Self {
            inner,
            max,
            creating: Mutex::new(()),
        }
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
        // Outside `watching`, no answer reads the mark.
        REFUSED_AT_CAP
            .try_with(|refused| refused.set(true))
            .unwrap_or_default();
        SessionsError::Full
    }

    /// `answering`, the answer to one request, and whether a manager refused
    /// a session at the cap while it ran: rmcp creates and restores the
    /// request's session on the task answering it.
    pub(crate) async fn watching<F: Future>(answering: F) -> (F::Output, bool) {
        REFUSED_AT_CAP
            .scope(Cell::new(false), async {
                let answer = answering.await;
                (answer, REFUSED_AT_CAP.with(Cell::get))
            })
            .await
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

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        let _creating = self.creating.lock().await;
        if self.is_full().await {
            return Err(self.full());
        }
        said("create_session", self.inner.create_session().await)
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        said(
            "initialize_session",
            self.inner.initialize_session(id, message).await,
        )
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        said("has_session", self.inner.has_session(id).await)
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        said("close_session", self.inner.close_session(id).await)
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        said("create_stream", self.inner.create_stream(id, message).await)
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        said(
            "accept_message",
            self.inner.accept_message(id, message).await,
        )
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
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
    /// present takes none.
    async fn restore_session(
        &self,
        id: SessionId,
    ) -> Result<RestoreOutcome<Self::Transport>, Self::Error> {
        let _creating = self.creating.lock().await;
        if !said("has_session", self.inner.has_session(&id).await)? && self.is_full().await {
            return Err(self.full());
        }
        said("restore_session", self.inner.restore_session(id).await)
    }

    fn event_store(&self) -> Option<Arc<dyn EventStore>> {
        self.inner.event_store()
    }
}

/// The app's `SessionStore`, each failure said: rmcp's own lines name the
/// session.
pub(crate) struct ReportedStore(pub(crate) Arc<dyn SessionStore>);

#[async_trait::async_trait]
impl SessionStore for ReportedStore {
    /// rmcp answers a failed load `500`.
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        self.0.load(session_id).await.inspect_err(|error| {
            tracing::error!(
                target: crate::TARGET,
                call = "load",
                error = %error_message(error.as_ref()),
                "mcp session store failed",
            );
        })
    }

    /// rmcp serves a session it failed to store, which another instance then
    /// cannot restore.
    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        self.0.store(session_id, state).await.inspect_err(|error| {
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
        self.0.delete(session_id).await.inspect_err(|error| {
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

    fn one_place() -> BoundedSessions {
        BoundedSessions::new(SessionConfig::default(), 1)
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
        let store = ReportedStore(Arc::new(Down));
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
}
