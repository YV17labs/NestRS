//! HTTP bridges (feature `http`) — request-boundary interceptor that installs
//! the ambient [`Executor`](crate::Executor), plus the [`Bind`] extractor that
//! turns a path id into the loaded, authorized entity.

mod bind;
mod interceptor;

pub use bind::Bind;
pub use interceptor::DbContext;
