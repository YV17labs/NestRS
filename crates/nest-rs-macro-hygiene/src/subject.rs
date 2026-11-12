//! `#[subject]` — its `Subject` impl, which every edge's posture reads, resolves
//! through the umbrella alone; each edge's witness gates an
//! operation on it under `authz`.

use nest_rs::authz::subject;

/// May export the hygiene reports: a capability no table holds.
#[subject]
pub struct HygieneExports;
