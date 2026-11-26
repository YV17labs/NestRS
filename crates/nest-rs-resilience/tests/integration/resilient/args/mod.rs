//! Covers the macro crate's `src/args.rs`, what each key a method writes makes
//! of its run: the predicates saying how an answer counts (`judged`), the
//! fallback answering in place of a run's error (`fallback`), the attempt's
//! token in the body (`cancel`), and all of them on each receiver and host
//! (`keyed`).

use super::*;

mod cancel;
mod fallback;
mod judged;
mod keyed;
