//! [`BoundedSessions`] — rmcp's in-memory sessions, held to a maximum.

use std::sync::Arc;

use futures_util::Stream;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
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
    /// `inner`, holding at most `max` live sessions.
    pub(crate) fn new(inner: LocalSessionManager, max: usize) -> Self {
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
