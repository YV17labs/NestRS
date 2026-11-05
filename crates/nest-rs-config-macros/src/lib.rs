//! The `#[config]` decorator, re-exported by `nest-rs-config`.
#![warn(missing_docs)]
#![doc(test(attr(deny(warnings), allow(dead_code, unused_variables))))]

use proc_macro::TokenStream;

mod config;
mod overlay;

/// The `#[config(namespace = "…")]` decorator: implements `Namespaced`, files
/// the namespace with the link-time registry (`nest_rs_config::unclaimed`) —
/// a second struct declaring it is refused at the read of either — and writes
/// `impl Config`, whose `from_env` reads each field under its name in
/// SCREAMING_SNAKE through the reader its type chooses.
///
/// Must sit **above** the derives so it sees them intact. `namespace` must be
/// a non-empty lowercase string. It carries the `Validate` derive itself;
/// `validate = "manual"` suppresses it for a config writing `impl Validate` by
/// hand, and `from_env = "manual"` the `impl Config` for one writing its overlay
/// by hand. A field's own `#[config(…)]` names its reader: `bounds = RANGE` for a
/// duration or a count, which binds the range to the field as the associated
/// `BoundedSetting` named for it, `json`, or `with = path`, handed the field's
/// binding. Every other field is bound too, as the associated `Field` named for
/// it, its key and its path, so a sentence naming a field reads them.
#[proc_macro_attribute]
pub fn config(args: TokenStream, input: TokenStream) -> TokenStream {
    ::nest_rs_codegen::reroot(config::config(args, input).into()).into()
}
