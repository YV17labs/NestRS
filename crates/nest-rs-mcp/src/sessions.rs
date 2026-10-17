//! [`BoundedSessions`] — rmcp's in-memory sessions, held to a maximum.

use std::sync::Arc;

use futures_util::Stream;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_server::session::local::{LocalSessionManager, SessionConfig};
use rmcp::transport::streamable_http_server::session::{
    EventStore, RestoreOutcome, ServerSseMessage, SessionId, SessionManager,
};
use tokio::sync::Mutex;

use crate::error::SessionsError;

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

    /// The most sessions this manager holds at once.
    pub(crate) fn max(&self) -> usize {
        self.max
    }

    /// Whether a new session would be refused now.
    pub(crate) async fn is_full(&self) -> bool {
        self.inner.sessions.read().await.len() >= self.max
    }
}

impl SessionManager for BoundedSessions {
    type Error = SessionsError;
    type Transport = <LocalSessionManager as SessionManager>::Transport;

    async fn create_session(&self) -> Result<(SessionId, Self::Transport), Self::Error> {
        let _creating = self.creating.lock().await;
        if self.is_full().await {
            return Err(SessionsError::Full);
        }
        Ok(self.inner.create_session().await?)
    }

    async fn initialize_session(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<ServerJsonRpcMessage, Self::Error> {
        Ok(self.inner.initialize_session(id, message).await?)
    }

    async fn has_session(&self, id: &SessionId) -> Result<bool, Self::Error> {
        Ok(self.inner.has_session(id).await?)
    }

    async fn close_session(&self, id: &SessionId) -> Result<(), Self::Error> {
        Ok(self.inner.close_session(id).await?)
    }

    async fn create_stream(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.create_stream(id, message).await?)
    }

    async fn accept_message(
        &self,
        id: &SessionId,
        message: ClientJsonRpcMessage,
    ) -> Result<(), Self::Error> {
        Ok(self.inner.accept_message(id, message).await?)
    }

    async fn create_standalone_stream(
        &self,
        id: &SessionId,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.create_standalone_stream(id).await?)
    }

    async fn resume(
        &self,
        id: &SessionId,
        last_event_id: String,
    ) -> Result<impl Stream<Item = ServerSseMessage> + Send + Sync + 'static, Self::Error> {
        Ok(self.inner.resume(id, last_event_id).await?)
    }

    /// A restored session takes a place like a created one; one already
    /// present takes none.
    async fn restore_session(
        &self,
        id: SessionId,
    ) -> Result<RestoreOutcome<Self::Transport>, Self::Error> {
        let _creating = self.creating.lock().await;
        if !self.inner.has_session(&id).await? && self.is_full().await {
            return Err(SessionsError::Full);
        }
        Ok(self.inner.restore_session(id).await?)
    }

    fn event_store(&self) -> Option<Arc<dyn EventStore>> {
        self.inner.event_store()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_place() -> BoundedSessions {
        BoundedSessions::new(SessionConfig::default(), 1)
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
