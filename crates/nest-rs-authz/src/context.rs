//! The caller's [`Ability`] as ambient, request-scoped state: the HTTP surface
//! installs it for the handler, `nest-rs-seaorm`'s `Repo` reads it back.

use std::future::Future;
use std::sync::Arc;

use crate::Ability;

tokio::task_local! {
    static ABILITY: Arc<Ability>;
}

/// The ambient [`Ability`], or `None` outside a request (or a request that
/// runs no authorization).
pub fn current_ability() -> Option<Arc<Ability>> {
    ABILITY.try_with(Arc::clone).ok()
}

/// Run `fut` with `ability` installed as the ambient, task-local capability set.
pub async fn with_ability<F: Future>(ability: Arc<Ability>, fut: F) -> F::Output {
    ABILITY.scope(ability, fut).await
}
