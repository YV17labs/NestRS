//! The in-band guard stage: each guard's [`Guard::check`] on a unit an in-band
//! edge carries.

use nest_rs_core::__private::{Pending, ResolvedLayer, take_pending};
use nest_rs_core::UnitContext;

use crate::{Denial, Guard};

/// Run every guard of `chain` on the unit `cx` describes through its in-band
/// entry, [`Guard::check`], in order: what they attached, for the edge to
/// install once the stage passed. The first refusal stops the stage and comes
/// back beside the name of the guard that raised it, as raised, for the edge to
/// file and render.
pub async fn run_stage(
    chain: &[ResolvedLayer<dyn Guard>],
    cx: &mut UnitContext<'_>,
) -> Result<Pending, (&'static str, Denial)> {
    for entry in chain {
        // `as_ref()`: dispatch on the erased guard — the `Guard for Arc<T>`
        // blanket would nest a second boxed future per check.
        entry
            .layer
            .as_ref()
            .check(cx)
            .await
            .map_err(|denial| (entry.name, denial))?;
    }
    Ok(take_pending(cx))
}
