//! [`ParentProbe`]: whether the parent a `#[field_resolver]` reads answers the
//! caller, asked before the resolver runs. A wire row's columns each check it where
//! they resolve; a resolver reads them in Rust, so a scalar it derives from a row
//! judged for someone else, or one the caller may not read, would ship. The
//! inherent arm exists for a wire row alone; [`ParentProbeFallback`] passes any
//! other parent.

use async_graphql::{Context, Error};
use nest_rs_core::__private::{Site, WireRow};

use crate::filed_facts::first_in_operation;
use crate::operation::served_field;

/// The parent a field resolver at a site reads, for the compiler to be asked
/// whether it is a wire row.
pub struct ParentProbe<'a, P: ?Sized>(pub &'a P);

impl<P: WireRow + ?Sized> ParentProbe<'_, P> {
    /// The parent answers the caller, or the field refuses as its columns do: a
    /// row the caller may not read `FORBIDDEN`, one judged for someone else
    /// `INTERNAL`. The refusal's line is filed once per operation, naming `site`.
    pub fn judged(&self, ctx: &Context<'_>, site: &'static Site) -> Result<(), Error> {
        let Some(refused) = self.0.refusal() else {
            return Ok(());
        };
        let handler = site.handler();
        if first_in_operation(
            ctx,
            format!(
                "parent {} {} {}",
                refused.entity(),
                refused.reason(),
                handler.address()
            ),
        ) {
            refused.file_at(Some(handler), served_field(ctx, Some(site)));
        }
        Err(crate::problem_error(&refused.refusal().problem()))
    }
}

/// What every parent that is no wire row answers.
pub trait ParentProbeFallback {
    /// Nothing to judge.
    fn judged(&self, _ctx: &Context<'_>, _site: &'static Site) -> Result<(), Error> {
        Ok(())
    }
}

impl<P: ?Sized> ParentProbeFallback for ParentProbe<'_, P> {}
