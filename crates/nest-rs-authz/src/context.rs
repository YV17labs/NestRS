//! The caller's [`Ability`] as ambient, request-scoped state.
//!
//! A singleton service cannot hold per-request state, yet transparent
//! row-level filtering needs the caller's `Ability` reachable from inside a
//! service method without threading it through every signature. A task-local
//! bridges that: the HTTP surface installs it for the duration of the handler;
//! `nest-rs-seaorm`'s `Repo` reads it back. Outside a request the task-local
//! is unset and [`current_ability`] returns `None` (an unscoped query).

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

pub async fn with_ability<F: Future>(ability: Arc<Ability>, fut: F) -> F::Output {
    ABILITY.scope(ability, fut).await
}
