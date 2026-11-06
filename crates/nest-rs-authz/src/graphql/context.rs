//! The [`Ability`] a GraphQL field runs under — the one its unit carries,
//! which its class gate checked — GraphQL's denial shape, and what a request
//! already filed of those denials.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use nest_rs_core::{Edge, Problem, code};
use nest_rs_graphql::async_graphql::{Error, ErrorExtensions, Result};
use nest_rs_guards::Denial;

use crate::gate::Refusal;
use crate::{Ability, Action};

/// One refused selection: the subject, the action, the columns.
type RefusedSelection = (&'static str, Action, String);

/// The selections one request refused, each one fact however many parents repeat it:
/// its line must not grow with what the caller asks for. Built by the first refusal, so
/// a request refusing nothing pays nothing.
#[derive(Default)]
pub(crate) struct FiledRefusals(OnceLock<Mutex<HashSet<RefusedSelection>>>);

impl FiledRefusals {
    /// Whether this is the first time the request files this refusal.
    pub(crate) fn first(&self, subject: &'static str, action: Action, fields: &str) -> bool {
        self.0
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((subject, action, fields.to_owned()))
    }
}

/// The [`Ability`] the running field's unit carries — what the guards at its
/// request or at the field itself attached, the one its class gate checked. A
/// wiring defect when absent — no ability guard ran — refused opaquely and
/// filed at `error`, never a client error.
pub fn ability() -> Result<Arc<Ability>> {
    crate::current_ability().ok_or_else(unreached)
}

/// The fail-closed exit of a field no ability reached, every one funnelling here.
pub(super) fn unreached() -> Error {
    crate::gate::error_unreached(Refusal {
        remedy: Some(UNREACHED),
        ..Refusal::on(Edge::Graphql.as_str())
    });
    nest_rs_graphql::denial_to_graphql_error(Denial::internal(
        "no ability reached the GraphQL operation",
    ))
}

/// What an app whose `/graphql` attaches no ability is told to change.
const UNREACHED: &str = "attach the ability on /graphql's request — the app's global guard \
                         pool or EndpointGuards<GraphqlView, (AuthnGuard, AuthzGuard)> — and \
                         import AuthzModule";

/// A GraphQL `forbidden` error (code `FORBIDDEN`) — the one denial shape every
/// GraphQL refusal carries, whether the class gate or the row
/// `#[authorize(A, bind = S)]` loads emits it.
pub fn forbidden() -> Error {
    nest_rs_graphql::problem_error(&Problem::new(403, code::FORBIDDEN))
}

/// [`forbidden`] naming the response fields the caller's field grant refuses —
/// the answer to an operation that selected a column it may not read. The
/// `fields` extension is a list, so a name containing a comma stays one entry.
pub(crate) fn forbidden_fields(fields: &[String]) -> Error {
    let names = fields.to_vec();
    forbidden().extend_with(move |_, e| e.set("fields", names))
}
