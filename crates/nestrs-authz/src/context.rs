//! The caller's [`Ability`] as ambient, request-scoped state.
//!
//! A singleton service cannot hold per-request state, yet transparent row-level
//! filtering needs the caller's [`Ability`] reachable from inside a service
//! method (where the query runs) without threading it through every signature.
//! A task-local bridges that: the HTTP surface installs the ability for the
//! duration of the handler (see `nestrs-authz-http`'s `Authorize` shaper, which
//! runs *inside* the route's guards, so the ability the guard built is present),
//! and `nestrs-database`'s `Repo` reads it back via [`current_ability`] to scope every
//! read. Outside a request the task-local is unset and [`current_ability`]
//! returns `None` (an unscoped query).

use std::future::Future;
use std::sync::Arc;

use crate::Ability;

tokio::task_local! {
    static ABILITY: Arc<Ability>;
}

/// The ambient [`Ability`] for the current request, or `None` when none is
/// installed (a non-request context, or a request that runs no authorization).
pub fn current_ability() -> Option<Arc<Ability>> {
    ABILITY.try_with(Arc::clone).ok()
}

/// Run `fut` with `ability` installed as the ambient request ability, so
/// [`current_ability`] resolves to it anywhere within `fut` (the handler and the
/// services it calls, all on the same task).
pub async fn with_ability<F: Future>(ability: Arc<Ability>, fut: F) -> F::Output {
    ABILITY.scope(ability, fut).await
}
