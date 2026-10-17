//! The tenant a unit of work is served for: an authorization input, ambient
//! like any value a unit carries, and set only from a verified source.

use std::sync::Arc;

use crate::ambient::{AmbientValue, Continuation, ambient};

/// The tenant the running unit is served for, as its ambient value.
///
/// Built from a verified source alone — the principal an authentication guard
/// resolved, or the envelope a queue push sealed — never from a value a request
/// carries, a header included: what a caller may read is decided on it. Only
/// `nest-rs-authn` and `nest-rs-queue` build one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TenantId(String);

impl TenantId {
    /// The tenant `id` names; `None` for an empty one, which names no tenant.
    pub fn new(id: impl Into<String>) -> Option<Self> {
        let id = id.into();
        (!id.is_empty()).then_some(Self(id))
    }

    /// The id, as the principal or the envelope named it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Crosses every continuation, a job's envelope included, where it rides as the
/// `tenant_id` key: a job runs for the tenant it was pushed for.
impl AmbientValue for TenantId {
    fn carry(self: Arc<Self>, _into: Continuation) -> Option<Arc<Self>> {
        Some(self)
    }
}

/// The tenant this unit of work is served for — `None` where no verified
/// principal named one and no job carried one.
///
/// Unlike [`current_actor_id`](crate::current_actor_id), an authorization
/// input: the authentication guard makes it ambient for the request and every
/// continuation of it, and every job the request pushes runs for it.
pub fn current_tenant_id() -> Option<String> {
    ambient::<TenantId>().map(|tenant| tenant.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_scope::RequestContinuation;
    use crate::{Correlation, with_ambient, with_request_scope};

    fn acme() -> Arc<TenantId> {
        Arc::new(TenantId::new("acme").expect("a tenant"))
    }

    #[test]
    fn an_empty_id_names_no_tenant() {
        assert_eq!(TenantId::new(""), None);
        assert_eq!(TenantId::new("acme").map(|t| t.0), Some("acme".to_owned()));
    }

    #[tokio::test]
    async fn the_tenant_is_read_where_it_is_ambient_and_nowhere_else() {
        assert_eq!(current_tenant_id(), None, "no unit, no tenant");
        let seen = with_ambient(acme(), async { current_tenant_id() }).await;
        assert_eq!(seen.as_deref(), Some("acme"));
        assert_eq!(current_tenant_id(), None);
    }

    #[tokio::test]
    async fn the_tenant_crosses_every_continuation_a_jobs_envelope_included() {
        for into in [
            Continuation::Body,
            Continuation::Batch,
            Continuation::Task,
            Continuation::Commit,
            Continuation::Connection,
            Continuation::Envelope,
        ] {
            let carried = with_request_scope(None, Correlation::minted(None), async {
                with_ambient(acme(), async { RequestContinuation::capture(into) }).await
            })
            .await
            .expect("a unit is running where it was captured");
            assert_eq!(
                carried.enter(current_tenant_id).as_deref(),
                Some("acme"),
                "{into:?}"
            );
        }
    }
}
