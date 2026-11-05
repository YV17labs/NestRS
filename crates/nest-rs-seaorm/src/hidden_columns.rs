//! What `#[expose]` bounds the wire impls it emits on a row on, so that a row
//! crosses no wire and no other impl fits beside them. Any other shape reads as
//! `E0283` or fails at the emitted impl (`decisions/mask-provenance.md`).

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
