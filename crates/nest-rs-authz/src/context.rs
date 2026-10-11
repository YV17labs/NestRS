//! The caller's [`Ability`] as a value of the ambient context: a guard or a
//! bridge installs it for the unit, `nest-rs-seaorm`'s `Repo` reads it back.

use std::future::Future;
use std::sync::Arc;

use nest_rs_core::AmbientValue;

use crate::Ability;

/// The caller's rules travel wherever the unit continues, and never into a
/// job's envelope: a job is system work.
impl AmbientValue for Ability {}

/// The ambient [`Ability`], or `None` outside a request (or a request that
/// runs no authorization).
pub fn current_ability() -> Option<Arc<Ability>> {
    nest_rs_core::ambient::<Ability>()
}

/// Run `fut` with `ability` installed as the ambient capability set.
pub async fn with_ability<F: Future>(ability: Arc<Ability>, fut: F) -> F::Output {
    nest_rs_core::with_ambient(ability, fut).await
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{Continuation, RequestContinuation};

    use super::*;
    use crate::AbilityBuilder;

    fn ability() -> Arc<Ability> {
        Arc::new(
            AbilityBuilder::new()
                .build()
                .expect("an empty rule set builds"),
        )
    }

    #[tokio::test]
    async fn the_ability_crosses_every_continuation_but_a_jobs_envelope() {
        let ability = ability();
        for into in [
            Continuation::Body,
            Continuation::Batch,
            Continuation::Task,
            Continuation::Commit,
            Continuation::Connection,
            Continuation::Envelope,
        ] {
            let carried = with_ability(Arc::clone(&ability), async {
                RequestContinuation::capture(into)
            })
            .await
            .expect("installed");
            let crossed = carried.enter(current_ability);
            match into {
                Continuation::Envelope => assert!(crossed.is_none(), "a job is system work"),
                _ => assert!(
                    crossed.is_some_and(|crossed| Arc::ptr_eq(&crossed, &ability)),
                    "{into:?} carries the caller's own ability",
                ),
            }
        }
    }
}
