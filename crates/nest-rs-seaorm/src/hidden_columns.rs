//! What `#[expose]` bounds the wire impls it emits on a row on, so that a row
//! crosses no wire and no other impl fits beside them.
//!
//! No impl of the trait exists or can be written: everything the bound names is
//! foreign to the crate an expansion lands in, so the orphan rule refuses one
//! there, and rustc reports an impl beside the emitted one as `E0119` rather than
//! `type annotations needed`. The lifetime keeps the bound from reading as a
//! trivially false one, refused at the emitted impl
//! (`decisions/mask-provenance.md`).

use std::marker::PhantomData;

/// The columns of a row, which reach a wire only through its exposed type.
pub struct HiddenColumns<'a>(PhantomData<&'a ()>);

/// What a row's wire impls ask of [`HiddenColumns`], and nothing answers;
/// `Reply` is `fn(Model) -> Wire`, which names the exposed type in rustc's sentence
/// and stays foreign where that type alone would be local.
#[diagnostic::on_unimplemented(
    message = "a row crosses no wire: `{Reply}` builds what a reply answers, and a write takes the input `#[expose(input(..))]` declares",
    label = "this carries every column, hidden ones included"
)]
pub trait AModelCrossesNoWire<Reply> {
    /// Stands for the body of a wire impl, which no bound lets anything call.
    fn refused() -> !;
}
