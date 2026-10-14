//! The caller's [`Ability`] as a value of the ambient context: the ability
//! guard attaches it for the unit, `nest-rs-seaorm`'s `Repo` reads it back.

use std::future::Future;
use std::sync::Arc;

use nest_rs_core::__private::UnmaskedReply;
use nest_rs_core::AmbientValue;

use crate::Ability;

/// The caller's rules travel wherever the unit continues, and never into a
/// job's envelope: a job is system work.
impl AmbientValue for Ability {}

/// The [`Ability`] the running unit's reads run under, or `None` outside a
/// request, in a request that runs no authorization, and on a route whose reply
/// no response mask shapes: there the ability the guard attached is withheld,
/// so a read the mask would have scoped fails closed instead of answering
/// unmasked.
pub fn current_ability() -> Option<Arc<Ability>> {
    if nest_rs_core::ambient::<UnmaskedReply>().is_some() {
        return None;
    }
    nest_rs_core::ambient::<Ability>()
}

/// Run `fut` with `ability` installed as the ambient capability set.
pub fn with_ability<F: Future>(ability: Arc<Ability>, fut: F) -> impl Future<Output = F::Output> {
    nest_rs_core::with_ambient(ability, fut)
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

    #[test]
    fn an_install_holds_the_unit_it_runs_once() {
        let heavy = || async {
            let held = [0u8; 32 * 1024];
            tokio::task::yield_now().await;
            std::hint::black_box(&held);
        };
        let alone = std::mem::size_of_val(&heavy());
        let size = std::mem::size_of_val(&with_ability(ability(), heavy()));
        assert!(
            size < alone + 1024,
            "with_ability weighs {size} bytes around a {alone}-byte unit",
        );
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
