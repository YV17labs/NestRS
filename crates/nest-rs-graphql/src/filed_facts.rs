//! What one operation's fields already filed: a fact they repeat under every parent is
//! one line, since what a caller asks for must not grow what the log holds. A socket's
//! operations are each their own, so what a caller probes over one files per probe.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use async_graphql::Context;

/// Held in each operation's data. Built by the first line, so an operation filing none
/// pays for one empty value.
#[derive(Debug, Default)]
pub(crate) struct FiledFacts(OnceLock<Mutex<HashSet<String>>>);

impl FiledFacts {
    pub(crate) fn first(&self, fact: String) -> bool {
        self.0
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(fact)
    }
}

/// Whether the operation `ctx` runs has yet to file `fact`. A schema no app composed
/// keeps no record, so there each files its own.
pub fn first_in_operation(ctx: &Context<'_>, fact: String) -> bool {
    ctx.data_opt::<Arc<FiledFacts>>()
        .is_none_or(|filed| filed.first(fact))
}

#[cfg(test)]
mod tests {
    use super::FiledFacts;

    #[test]
    fn a_fact_is_first_once() {
        let filed = FiledFacts::default();

        assert!(filed.first("one".to_owned()));
        assert!(!filed.first("one".to_owned()));
        assert!(filed.first("another".to_owned()));
    }
}
